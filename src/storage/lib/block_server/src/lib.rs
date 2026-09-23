// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::Error;
use block_protocol::{BlockFifoRequest, BlockFifoResponse};
use fblock::{BlockIoFlag, BlockOpcode, MAX_TRANSFER_UNBOUNDED};
use fidl_fuchsia_storage_block as fblock;
use fuchsia_async as fasync;
use fuchsia_async::epoch::{Epoch, EpochGuard};
use fuchsia_sync::{MappedMutexGuard, Mutex, MutexGuard};
use futures::{Future, FutureExt as _, TryStreamExt as _};
use slab::Slab;
use std::borrow::{Borrow, Cow};
use std::collections::BTreeMap;
use std::num::NonZero;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use storage_device::buffer::Buffer;

pub mod async_interface;
pub mod c_interface;
pub mod callback_interface;
mod mapper;
pub mod verifier;

#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod decompression_tests;

pub(crate) const FIFO_MAX_REQUESTS: usize = 64;

type TraceFlowId = Option<NonZero<u64>>;

#[derive(Clone, Debug)]
pub enum DeviceInfo {
    /// A raw non-partition block device.
    Block(BlockInfo),
    /// A static partition with fixed physical mappings.
    Partition(PartitionInfo),
    /// A dynamic volume whose slice/block count is queried via get_volume_info.
    Volume(VolumeInfo),
}

impl DeviceInfo {
    pub fn label(&self) -> &str {
        match self {
            Self::Block(BlockInfo { .. }) => "",
            Self::Partition(PartitionInfo { name, .. }) => name,
            Self::Volume(VolumeInfo { name, .. }) => name,
        }
    }
    pub fn device_flags(&self) -> fblock::DeviceFlag {
        match self {
            Self::Block(BlockInfo { device_flags, .. }) => *device_flags,
            Self::Partition(PartitionInfo { device_flags, .. }) => *device_flags,
            Self::Volume(VolumeInfo { device_flags, .. }) => *device_flags,
        }
    }

    /// Returns the block count of the device or partition.
    /// Returns None for dynamic volumes (whose size needs to be queried from the volume manager).
    pub fn block_count(&self) -> Option<u64> {
        match self {
            Self::Block(BlockInfo { block_count, .. }) => Some(*block_count),
            Self::Partition(PartitionInfo { block_count, .. }) => Some(*block_count),
            Self::Volume(VolumeInfo { .. }) => None,
        }
    }

    pub fn max_transfer_blocks(&self) -> Option<NonZero<u32>> {
        match self {
            Self::Block(BlockInfo { max_transfer_blocks, .. }) => max_transfer_blocks.clone(),
            Self::Partition(PartitionInfo { max_transfer_blocks, .. }) => {
                max_transfer_blocks.clone()
            }
            Self::Volume(VolumeInfo { max_transfer_blocks, .. }) => max_transfer_blocks.clone(),
        }
    }

    fn max_transfer_size(&self, block_size: u32) -> u32 {
        if let Some(max_blocks) = self.max_transfer_blocks() {
            max_blocks.get() * block_size
        } else {
            MAX_TRANSFER_UNBOUNDED
        }
    }

    pub fn type_guid(&self) -> Option<[u8; 16]> {
        match self {
            Self::Partition(PartitionInfo { type_guid, .. }) => Some(*type_guid),
            Self::Volume(VolumeInfo { type_guid, .. }) => Some(*type_guid),
            Self::Block(_) => None,
        }
    }

    pub fn instance_guid(&self) -> Option<[u8; 16]> {
        match self {
            Self::Partition(PartitionInfo { instance_guid, .. }) => Some(*instance_guid),
            Self::Volume(VolumeInfo { instance_guid, .. }) => Some(*instance_guid),
            Self::Block(_) => None,
        }
    }
}

/// Information associated with non-partition block devices.
#[derive(Clone, Default, Debug)]
pub struct BlockInfo {
    pub device_flags: fblock::DeviceFlag,
    pub block_count: u64,
    pub max_transfer_blocks: Option<NonZero<u32>>,
}

/// Information associated with a block device that is also a partition.
#[derive(Clone, Default, Debug)]
pub struct PartitionInfo {
    /// The device flags reported by the underlying device.
    pub device_flags: fblock::DeviceFlag,
    pub max_transfer_blocks: Option<NonZero<u32>>,
    /// This is None for partitions which have multiple logical extents, in which case the
    /// start_block_offset is not meaningful and potentially confusing.
    pub start_block_offset: Option<u64>,
    pub block_count: u64,
    pub type_guid: [u8; 16],
    pub instance_guid: [u8; 16],
    pub name: String,
    /// This can be None for partitions which are composed of multiple partitions (e.g. an
    /// overlay partition in GPT).
    pub flags: Option<u64>,
}

/// Information associated with a dynamic volume (such as FVM volumes).
#[derive(Clone, Default, Debug)]
pub struct VolumeInfo {
    /// The device flags reported by the underlying device.
    pub device_flags: fblock::DeviceFlag,
    pub max_transfer_blocks: Option<NonZero<u32>>,
    pub type_guid: [u8; 16],
    pub instance_guid: [u8; 16],
    pub name: String,
    pub flags: u64,
}

/// We internally keep track of active requests, so that when the server is torn down, we can
/// deallocate all of the resources for pending requests.
struct ActiveRequest<S> {
    session: S,
    group_or_request: GroupOrRequest,
    trace_flow_id: TraceFlowId,
    _epoch_guard: EpochGuard<'static>,
    status: Result<(), zx::Status>,
    count: u32,
    req_id: Option<u32>,
    decompression_info: Option<DecompressionInfo>,
}

struct DecompressionInfo {
    // This is the range of compressed bytes in receiving buffer.
    compressed_range: Range<usize>,

    // This is the range in the target VMO where we will write uncompressed bytes.
    uncompressed_range: Range<u64>,

    bytes_so_far: u64,
    mapping: Arc<VmoMapping>,
    buffer: Option<Buffer<'static>>,
}

impl DecompressionInfo {
    /// Returns the uncompressed slice.
    fn uncompressed_slice(&self) -> *mut [u8] {
        std::ptr::slice_from_raw_parts_mut(
            (self.mapping.base + self.uncompressed_range.start as usize) as *mut u8,
            (self.uncompressed_range.end - self.uncompressed_range.start) as usize,
        )
    }
}

pub struct ActiveRequests<S>(Mutex<ActiveRequestsInner<S>>);

impl<S> Default for ActiveRequests<S> {
    fn default() -> Self {
        Self(Mutex::new(ActiveRequestsInner { requests: Slab::default() }))
    }
}

impl<S> ActiveRequests<S> {
    fn complete_and_take_response(
        &self,
        request_id: RequestId,
        status: Result<(), zx::Status>,
    ) -> Option<(S, BlockFifoResponse)> {
        self.0.lock().complete_and_take_response(request_id, status)
    }

    fn request(&self, request_id: RequestId) -> MappedMutexGuard<'_, ActiveRequest<S>> {
        MutexGuard::map(self.0.lock(), |i| &mut i.requests[request_id.0])
    }
}

struct ActiveRequestsInner<S> {
    requests: Slab<ActiveRequest<S>>,
}

// Keeps track of all the requests that are currently being processed
impl<S> ActiveRequestsInner<S> {
    /// Completes a request.
    fn complete(&mut self, request_id: RequestId, status: Result<(), zx::Status>) {
        let group = &mut self.requests[request_id.0];

        group.count = group.count.checked_sub(1).unwrap();
        if status.is_err() && group.status.is_ok() {
            group.status = status
        }

        fuchsia_trace::duration!(
            "storage",
            "block_server::finish_transaction",
            "request_id" => request_id.0,
            "group_completed" => group.count == 0,
            "status" => zx::Status::result_into_raw(status));
        if let Some(trace_flow_id) = group.trace_flow_id {
            fuchsia_trace::flow_step!(
                "storage",
                "block_server::finish_request",
                trace_flow_id.get().into()
            );
        }

        if group.count == 0
            && group.status.is_ok()
            && let Some(info) = &mut group.decompression_info
        {
            struct RawDCtx(std::ptr::NonNull<zstd::zstd_safe::zstd_sys::ZSTD_DCtx>);

            thread_local! {
                static RAW_DECOMPRESSOR: std::cell::RefCell<RawDCtx> = {
                    // SAFETY: Creating a new ZSTD decompression context does not borrow or capture
                    // any external memory.
                    let raw_ptr = unsafe { zstd::zstd_safe::zstd_sys::ZSTD_createDCtx() };
                    let ptr = std::ptr::NonNull::new(raw_ptr).expect("ZSTD_createDCtx failed");
                    std::cell::RefCell::new(RawDCtx(ptr))
                };
            }

            impl Drop for RawDCtx {
                fn drop(&mut self) {
                    // SAFETY: `self.0` is non-null and was allocated via `ZSTD_createDCtx`.
                    unsafe {
                        zstd::zstd_safe::zstd_sys::ZSTD_freeDCtx(self.0.as_ptr());
                    }
                }
            }

            RAW_DECOMPRESSOR.with_borrow_mut(|decompressor| {
                let dctx = decompressor.0.as_ptr();
                let target = info.uncompressed_slice();
                let buffer = info.buffer.take().unwrap();
                let source = buffer.subslice(info.compressed_range.clone());

                // SAFETY: `target` points to valid uncompressed destination memory in the VMO
                // mapping. `source` points to valid compressed memory within `buffer`.
                unsafe {
                    let result = zstd::zstd_safe::zstd_sys::ZSTD_decompressDCtx(
                        dctx,
                        target as *mut u8 as *mut std::os::raw::c_void,
                        target.len(),
                        source.as_ptr() as *const std::os::raw::c_void,
                        source.len(),
                    );
                    if zstd::zstd_safe::zstd_sys::ZSTD_isError(result) != 0 {
                        let error = zstd::zstd_safe::get_error_name(result);
                        log::warn!(error:?; "Decompression error");
                        group.status = Err(zx::Status::IO_DATA_INTEGRITY);
                    }
                }
            });
        }
    }

    /// Takes the response if all requests are finished.
    fn take_response(&mut self, request_id: RequestId) -> Option<(S, BlockFifoResponse)> {
        let group = &self.requests[request_id.0];
        match group.req_id {
            Some(reqid) if group.count == 0 => {
                let group = self.requests.remove(request_id.0);
                Some((
                    group.session,
                    BlockFifoResponse {
                        status: zx::Status::result_into_raw(group.status),
                        reqid,
                        group: group.group_or_request.group_id().unwrap_or(0),
                        ..Default::default()
                    },
                ))
            }
            _ => None,
        }
    }

    /// Competes the request and returns a response if the request group is finished.
    fn complete_and_take_response(
        &mut self,
        request_id: RequestId,
        status: Result<(), zx::Status>,
    ) -> Option<(S, BlockFifoResponse)> {
        self.complete(request_id, status);
        self.take_response(request_id)
    }
}

/// BlockServer is an implementation of fuchsia.hardware.block.partition.Partition.
/// cbindgen:no-export
pub struct BlockServer<SM: SessionManager> {
    block_size: u32,
    orchestrator: Arc<SM::Orchestrator>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockOffsetMapping {
    pub target_block_offset: u64,
    pub length: u64,
}

/// Merges physically contiguous mappings into single logical mappings.  This is useful because it
/// prevents requests from being unnecessarily split if they span the two mappings.
pub fn coalesce_mappings(raw_mappings: Vec<BlockOffsetMapping>) -> Vec<BlockOffsetMapping> {
    let mut mappings: Vec<BlockOffsetMapping> = Vec::with_capacity(raw_mappings.len());
    for m in raw_mappings {
        if let Some(last) = mappings.last_mut()
            && last.target_block_offset + last.length == m.target_block_offset
        {
            last.length += m.length;
        } else {
            mappings.push(m);
        }
    }
    mappings
}

/// Remaps the offset of block requests based on an internal map of contiguous logical extents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OffsetMap {
    mappings: Vec<BlockOffsetMapping>,
}

impl OffsetMap {
    /// Creates a new `OffsetMap` from a list of `BlockOffsetMapping`s.
    /// Returns `INVALID_ARGS` if any mapping length is zero, or if logical/target block offsets
    /// overflow `u64`.
    pub fn new(mappings: Vec<BlockOffsetMapping>) -> Result<Self, zx::Status> {
        let mut total: u64 = 0;
        for m in &mappings {
            if m.length == 0 {
                return Err(zx::Status::INVALID_ARGS);
            }
            m.target_block_offset.checked_add(m.length).ok_or(zx::Status::INVALID_ARGS)?;
            total = total.checked_add(m.length).ok_or(zx::Status::INVALID_ARGS)?;
        }
        Ok(Self { mappings })
    }

    /// Creates an empty `OffsetMap`.
    pub fn empty() -> Self {
        Self { mappings: Vec::new() }
    }

    /// Maps `logical_offset` to `(target_block_offset, len)`, where `len` is the total number of
    /// blocks which can be addressed from `target_block_offset` onwards.
    ///
    /// For example: if you have logical offset 0 pointing to physical extent 1000..1010, and you
    /// search for offset 5, the return value will be `Some((1005, 5))`.
    pub fn map(&self, logical_offset: u64) -> Option<(u64, u32)> {
        let mut current_logical_start = 0;
        for mapping in &self.mappings {
            let current_logical_end = current_logical_start + mapping.length;
            if logical_offset < current_logical_end {
                let delta = logical_offset - current_logical_start;
                let dev_offset = mapping.target_block_offset + delta;
                let len = u32::try_from(current_logical_end - logical_offset).unwrap_or(u32::MAX);
                return Some((dev_offset, len));
            }
            current_logical_start = current_logical_end;
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }

    pub fn total_blocks(&self) -> u64 {
        self.mappings.iter().map(|m| m.length).sum()
    }

    /// Returns true if the block range `[offset, offset + length)` falls entirely within this
    /// map's total blocks. If the map is empty, returns true (no range restriction).
    pub fn are_blocks_within_source_range(&self, (offset, length): (u64, u32)) -> bool {
        if self.is_empty() {
            return true;
        }
        let total = self.total_blocks();
        offset <= total && total - offset >= length as u64
    }

    pub fn mappings(&self) -> &[BlockOffsetMapping] {
        &self.mappings
    }
}

impl TryFrom<&fblock::BlockOffsetMapping> for BlockOffsetMapping {
    type Error = zx::Status;

    fn try_from(wire: &fblock::BlockOffsetMapping) -> Result<Self, Self::Error> {
        if wire.length == 0 {
            return Err(zx::Status::INVALID_ARGS);
        }
        wire.target_block_offset.checked_add(wire.length).ok_or(zx::Status::OUT_OF_RANGE)?;
        Ok(BlockOffsetMapping {
            target_block_offset: wire.target_block_offset,
            length: wire.length,
        })
    }
}

impl TryFrom<fblock::BlockOffsetMapping> for BlockOffsetMapping {
    type Error = zx::Status;

    fn try_from(wire: fblock::BlockOffsetMapping) -> Result<Self, Self::Error> {
        BlockOffsetMapping::try_from(&wire)
    }
}

impl From<&BlockOffsetMapping> for fblock::BlockOffsetMapping {
    fn from(m: &BlockOffsetMapping) -> Self {
        fblock::BlockOffsetMapping { target_block_offset: m.target_block_offset, length: m.length }
    }
}

impl From<BlockOffsetMapping> for fblock::BlockOffsetMapping {
    fn from(m: BlockOffsetMapping) -> Self {
        fblock::BlockOffsetMapping::from(&m)
    }
}

impl TryFrom<&[fblock::BlockOffsetMapping]> for OffsetMap {
    type Error = zx::Status;

    fn try_from(wire: &[fblock::BlockOffsetMapping]) -> Result<Self, Self::Error> {
        let raw_mappings: Vec<BlockOffsetMapping> =
            wire.iter().map(BlockOffsetMapping::try_from).collect::<Result<_, _>>()?;
        OffsetMap::new(raw_mappings)
    }
}

impl TryFrom<Vec<fblock::BlockOffsetMapping>> for OffsetMap {
    type Error = zx::Status;

    fn try_from(wire: Vec<fblock::BlockOffsetMapping>) -> Result<Self, Self::Error> {
        OffsetMap::try_from(wire.as_slice())
    }
}

impl From<&OffsetMap> for Vec<fblock::BlockOffsetMapping> {
    fn from(offset_map: &OffsetMap) -> Self {
        offset_map.mappings.iter().map(fblock::BlockOffsetMapping::from).collect()
    }
}

// Methods take Arc<Self> rather than &self because of
// https://github.com/rust-lang/rust/issues/42940.
pub trait SessionManager: 'static {
    /// The Orchestrator is an object that holds the `SessionManager` and any other state that needs
    /// to be shared between sessions.  It is responsible for keeping the `SessionManager` alive.
    /// We use this type instead of directly holding an Arc<SessionManager> in BlockServer, to avoid
    /// nested Arcs in concrete implementations which need to keep additional state.
    type Orchestrator: Borrow<Self> + Send + Sync;

    const SUPPORTS_DECOMPRESSION: bool;

    type Session;

    /// Returns true iff `a` and `b` identify the same session.  Used to scope
    /// group-ID lookups in the shared `active_requests` slab to the originating
    /// session.
    fn session_eq(a: &Self::Session, b: &Self::Session) -> bool;

    fn on_attach_vmo(
        orchestrator: Arc<Self::Orchestrator>,
        vmo: &Arc<zx::Vmo>,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send;

    /// Creates a new session to handle `stream`.
    ///
    /// The returned future should run until the session completes, for example when the client end
    /// closes.
    ///
    /// `offset_map` is an optional client-provided map to adjust the offset/length of FIFO
    /// requests.  If the implementation supports mapping requests, it must forward this back to
    /// [`SessionHelper::new`].
    fn open_session(
        orchestrator: Arc<Self::Orchestrator>,
        stream: fblock::SessionRequestStream,
        offset_map: OffsetMap,
        block_size: u32,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Called to get block/partition information for Block::GetInfo, Partition::GetTypeGuid, etc.
    fn get_info(&self) -> Cow<'_, DeviceInfo>;

    /// Called to handle the GetVolumeInfo FIDL call.
    fn get_volume_info(
        &self,
    ) -> impl Future<Output = Result<(fblock::VolumeManagerInfo, fblock::VolumeInfo), zx::Status>> + Send
    {
        async { Err(zx::Status::NOT_SUPPORTED) }
    }

    /// Called to handle the QuerySlices FIDL call.
    fn query_slices(
        &self,
        _start_slices: &[u64],
    ) -> impl Future<Output = Result<Vec<fblock::VsliceRange>, zx::Status>> + Send {
        async { Err(zx::Status::NOT_SUPPORTED) }
    }

    /// Called to handle the Shrink FIDL call.
    fn extend(
        &self,
        _start_slice: u64,
        _slice_count: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send {
        async { Err(zx::Status::NOT_SUPPORTED) }
    }

    /// Called to handle the Shrink FIDL call.
    fn shrink(
        &self,
        _start_slice: u64,
        _slice_count: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send {
        async { Err(zx::Status::NOT_SUPPORTED) }
    }

    /// Opens a new mapper session.
    fn open_mapper_session(
        _orchestrator: Arc<Self::Orchestrator>,
        _session: fidl::endpoints::ServerEnd<fblock::MapperSessionMarker>,
        _mapping_vmo: zx::Vmo,
        _block_size: u32,
        _port: Option<zx::Port>,
        _delivery_queue: Option<zx::Vmo>,
    ) -> Result<impl Future<Output = Result<(), Error>> + Send, zx::Status> {
        Err::<std::future::Ready<Result<(), Error>>, _>(zx::Status::NOT_SUPPORTED)
    }

    /// Returns the active requests.
    fn active_requests(&self) -> &ActiveRequests<Self::Session>;
}

/// A helper trait for converting various types into an `Orchestrator`.
///
/// This exists to simplify [`BlockServer::new`].
pub trait IntoOrchestrator {
    type SM: SessionManager;

    fn into_orchestrator(self) -> Arc<<Self::SM as SessionManager>::Orchestrator>;
}

impl<SM: SessionManager> BlockServer<SM> {
    pub fn new(block_size: u32, orchestrator: impl IntoOrchestrator<SM = SM>) -> Self {
        Self { block_size, orchestrator: orchestrator.into_orchestrator() }
    }

    pub fn session_manager(&self) -> &SM {
        self.orchestrator.as_ref().borrow()
    }

    /// Called to process requests for fuchsia.storage.block.Block.
    pub async fn handle_requests(
        &self,
        mut requests: fblock::BlockRequestStream,
    ) -> Result<(), Error> {
        let scope = fasync::Scope::new();
        loop {
            match requests.try_next().await {
                Ok(Some(request)) => {
                    if let Some(session) = self.handle_request(request, &scope).await? {
                        scope.spawn(session.map(|_| ()));
                    }
                }
                Ok(None) => break,
                Err(error) => log::warn!(error:?; "Invalid request"),
            }
        }
        scope.await;
        Ok(())
    }

    /// Called to process requests for fuchsia.storage.block.Mapper.
    pub async fn handle_mapper_requests(
        &self,
        requests: fblock::MapperRequestStream,
    ) -> Result<(), Error> {
        Self::handle_mapper_requests_impl(self.orchestrator.clone(), self.block_size, requests)
            .await
    }

    async fn handle_mapper_requests_impl(
        orchestrator: Arc<SM::Orchestrator>,
        block_size: u32,
        mut requests: fblock::MapperRequestStream,
    ) -> Result<(), Error> {
        let scope = fasync::Scope::new();
        loop {
            match requests.try_next().await {
                Ok(Some(request)) => match request {
                    fblock::MapperRequest::OpenSession {
                        session,
                        mapping_vmo,
                        port,
                        delivery_queue,
                        responder,
                    } => {
                        match SM::open_mapper_session(
                            orchestrator.clone(),
                            session,
                            mapping_vmo,
                            block_size,
                            port,
                            delivery_queue,
                        ) {
                            Ok(fut) => {
                                responder.send(Ok(()))?;
                                scope.spawn(async move {
                                    if let Err(error) = fut.await {
                                        log::warn!(error:?; "Mapper session failed");
                                    }
                                });
                            }
                            Err(status) => {
                                responder.send(Err(status.into_raw()))?;
                            }
                        }
                    }
                    fblock::MapperRequest::_UnknownMethod { .. } => {}
                },
                Ok(None) => break,
                Err(error) => log::warn!(error:?; "Invalid mapper request"),
            }
        }
        scope.await;
        Ok(())
    }

    /// Processes a Block request.  If a new session task is created in response to the request,
    /// it is returned.
    async fn handle_request(
        &self,
        request: fblock::BlockRequest,
        scope: &fasync::Scope,
    ) -> Result<Option<impl Future<Output = Result<(), Error>> + Send + use<SM>>, Error> {
        match request {
            fblock::BlockRequest::GetInfo { responder } => {
                let info = self.device_info();
                let max_transfer_size = info.max_transfer_size(self.block_size);
                let (block_count, mut flags) = match info.as_ref() {
                    DeviceInfo::Block(BlockInfo { block_count, device_flags, .. }) => {
                        (*block_count, *device_flags)
                    }
                    DeviceInfo::Partition(partition_info) => {
                        (partition_info.block_count, partition_info.device_flags)
                    }
                    DeviceInfo::Volume(volume_info) => {
                        let volume_info_fidl = self.session_manager().get_volume_info().await?;
                        let block_count = volume_info_fidl.0.slice_size
                            * volume_info_fidl.1.partition_slice_count
                            / self.block_size as u64;
                        (block_count, volume_info.device_flags)
                    }
                };
                if SM::SUPPORTS_DECOMPRESSION {
                    flags |= fblock::DeviceFlag::ZSTD_DECOMPRESSION_SUPPORT;
                }
                responder.send(Ok(&fblock::BlockInfo {
                    block_count,
                    block_size: self.block_size,
                    max_transfer_size,
                    flags,
                }))?;
            }
            fblock::BlockRequest::OpenSession { session, control_handle: _ } => {
                return Ok(Some(SM::open_session(
                    self.orchestrator.clone(),
                    session.into_stream(),
                    OffsetMap::empty(),
                    self.block_size,
                )));
            }
            fblock::BlockRequest::OpenSessionWithOptions {
                session,
                mappings,
                control_handle: _,
            } => {
                let info = self.device_info();
                let offset_map: OffsetMap = match mappings.as_slice().try_into() {
                    Ok(map) => map,
                    Err(status) => {
                        session.close_with_epitaph(status)?;
                        return Ok(None);
                    }
                };
                if let Some(max) = info.block_count() {
                    for m in offset_map.mappings() {
                        if m.target_block_offset.checked_add(m.length).unwrap_or(u64::MAX) > max {
                            log::warn!("Invalid mapping for session: {m:?} (max blocks {max})");
                            session.close_with_epitaph(zx::Status::OUT_OF_RANGE)?;
                            return Ok(None);
                        }
                    }
                }
                return Ok(Some(SM::open_session(
                    self.orchestrator.clone(),
                    session.into_stream(),
                    offset_map,
                    self.block_size,
                )));
            }
            fblock::BlockRequest::ConnectMapper { server_end, responder } => {
                let orchestrator = self.orchestrator.clone();
                let block_size = self.block_size;
                scope.spawn(async move {
                    if let Err(e) = Self::handle_mapper_requests_impl(
                        orchestrator,
                        block_size,
                        server_end.into_stream(),
                    )
                    .await
                    {
                        log::warn!(e:?; "Error serving mapper requests");
                    }
                });
                let _ = responder.send(Ok(()));
            }
            fblock::BlockRequest::GetTypeGuid { responder } => {
                match self.device_info().type_guid() {
                    Some(guid) => {
                        responder.send(zx::sys::ZX_OK, Some(&fblock::Guid { value: guid }))?
                    }
                    None => responder.send(zx::sys::ZX_ERR_NOT_SUPPORTED, None)?,
                }
            }
            fblock::BlockRequest::GetInstanceGuid { responder } => {
                match self.device_info().instance_guid() {
                    Some(guid) => {
                        responder.send(zx::sys::ZX_OK, Some(&fblock::Guid { value: guid }))?
                    }
                    None => responder.send(zx::sys::ZX_ERR_NOT_SUPPORTED, None)?,
                }
            }
            fblock::BlockRequest::GetName { responder } => {
                let info = self.device_info();
                match info.as_ref() {
                    DeviceInfo::Partition(_) | DeviceInfo::Volume(_) => {
                        responder.send(zx::sys::ZX_OK, Some(info.label()))?;
                    }
                    _ => responder.send(zx::sys::ZX_ERR_NOT_SUPPORTED, None)?,
                }
            }
            fblock::BlockRequest::GetMetadata { responder } => {
                let device_info = self.device_info();
                match device_info.as_ref() {
                    DeviceInfo::Partition(info) => {
                        let mut type_guid =
                            fblock::Guid { value: [0u8; fblock::GUID_LENGTH as usize] };
                        type_guid.value.copy_from_slice(&info.type_guid);
                        let mut instance_guid =
                            fblock::Guid { value: [0u8; fblock::GUID_LENGTH as usize] };
                        instance_guid.value.copy_from_slice(&info.instance_guid);
                        let start_block_offset = info.start_block_offset;
                        let flags = info.flags;
                        responder.send(Ok(&fblock::PartitionInfo {
                            name: Some(info.name.clone()),
                            type_guid: Some(type_guid),
                            instance_guid: Some(instance_guid),
                            start_block_offset,
                            num_blocks: device_info.block_count(),
                            flags,
                            ..Default::default()
                        }))?;
                    }
                    DeviceInfo::Volume(info) => {
                        let mut type_guid =
                            fblock::Guid { value: [0u8; fblock::GUID_LENGTH as usize] };
                        type_guid.value.copy_from_slice(&info.type_guid);
                        let mut instance_guid =
                            fblock::Guid { value: [0u8; fblock::GUID_LENGTH as usize] };
                        instance_guid.value.copy_from_slice(&info.instance_guid);
                        responder.send(Ok(&fblock::PartitionInfo {
                            name: Some(info.name.clone()),
                            type_guid: Some(type_guid),
                            instance_guid: Some(instance_guid),
                            start_block_offset: None,
                            num_blocks: device_info.block_count(),
                            flags: Some(info.flags),
                            ..Default::default()
                        }))?;
                    }
                    _ => responder.send(Err(zx::sys::ZX_ERR_NOT_SUPPORTED))?,
                }
            }
            fblock::BlockRequest::QuerySlices { responder, start_slices } => {
                match self.session_manager().query_slices(&start_slices).await {
                    Ok(mut results) => {
                        let results_len = results.len();
                        assert!(results_len <= 16);
                        results.resize(16, fblock::VsliceRange { allocated: false, count: 0 });
                        responder.send(
                            zx::sys::ZX_OK,
                            &results.try_into().unwrap(),
                            results_len as u64,
                        )?;
                    }
                    Err(s) => {
                        responder.send(
                            s.into_raw(),
                            &[fblock::VsliceRange { allocated: false, count: 0 }; 16],
                            0,
                        )?;
                    }
                }
            }
            fblock::BlockRequest::GetVolumeInfo { responder, .. } => {
                match self.session_manager().get_volume_info().await {
                    Ok((manager_info, volume_info)) => {
                        responder.send(zx::sys::ZX_OK, Some(&manager_info), Some(&volume_info))?
                    }
                    Err(s) => responder.send(s.into_raw(), None, None)?,
                }
            }
            fblock::BlockRequest::Extend { responder, start_slice, slice_count } => {
                responder.send(zx::Status::result_into_raw(
                    self.session_manager().extend(start_slice, slice_count).await,
                ))?;
            }
            fblock::BlockRequest::Shrink { responder, start_slice, slice_count } => {
                responder.send(zx::Status::result_into_raw(
                    self.session_manager().shrink(start_slice, slice_count).await,
                ))?;
            }
            fblock::BlockRequest::Destroy { responder, .. } => {
                responder.send(zx::sys::ZX_ERR_NOT_SUPPORTED)?;
            }
        }
        Ok(None)
    }

    fn device_info(&self) -> Cow<'_, DeviceInfo> {
        self.session_manager().get_info()
    }
}

pub(crate) struct RegisteredVmo {
    pub vmo: Arc<zx::Vmo>,
    pub size: u64,
    pub mapping: Option<Arc<VmoMapping>>,
}

impl RegisteredVmo {
    /// Validates that a request range (`vmo_offset` to `vmo_offset + length`, in bytes) falls
    /// within the bounds of this VMO.
    fn validate_request(&self, vmo_offset: u64, length: u64) -> Result<(), zx::Status> {
        if vmo_offset > self.size || self.size - vmo_offset < length {
            Err(zx::Status::OUT_OF_RANGE)
        } else {
            Ok(())
        }
    }

    /// Returns the cached VMO mapping if available, or creates and caches a new mapping.
    fn get_or_create_mapping(&mut self) -> Result<Arc<VmoMapping>, zx::Status> {
        match &self.mapping {
            Some(mapping) => Ok(mapping.clone()),
            None => {
                let mapping = VmoMapping::new(&self.vmo, self.size as usize)?;
                self.mapping = Some(mapping.clone());
                Ok(mapping)
            }
        }
    }
}

struct SessionHelper<SM: SessionManager> {
    orchestrator: Arc<SM::Orchestrator>,
    offset_map: OffsetMap,
    max_transfer_blocks: Option<NonZero<u32>>,
    block_size: u32,
    peer_fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest>,
    vmos: Mutex<BTreeMap<u16, RegisteredVmo>>,
}

struct VmoMapping {
    base: usize,
    size: usize,
}

impl VmoMapping {
    fn new(vmo: &zx::Vmo, size: usize) -> Result<Arc<Self>, zx::Status> {
        Ok(Arc::new(Self {
            base: fuchsia_runtime::vmar_root_self()
                .map(0, vmo, 0, size, zx::VmarFlags::PERM_WRITE | zx::VmarFlags::PERM_READ)
                .inspect_err(|error| {
                    log::warn!(error:?, size; "VmoMapping: unable to map VMO");
                })?,
            size,
        }))
    }
}

impl Drop for VmoMapping {
    fn drop(&mut self) {
        // SAFETY: We mapped this in `VmoMapping::new`.
        unsafe {
            let _ = fuchsia_runtime::vmar_root_self().unmap(self.base, self.size);
        }
    }
}

enum HandleRequestResult {
    /// The request was handled successfully.
    Ok,
    /// The request closed the stream.  The caller must shut down the session, and must call the
    /// provided callback after the session is completely shut down.  The caller should assume that
    /// no further requests need to be handled once this is received.
    Closed(Box<dyn FnOnce() + Send + 'static>),
}

impl<SM: SessionManager> SessionHelper<SM> {
    fn new(
        orchestrator: Arc<SM::Orchestrator>,
        offset_map: OffsetMap,
        max_transfer_blocks: Option<NonZero<u32>>,
        block_size: u32,
    ) -> Result<(Self, zx::Fifo<BlockFifoRequest, BlockFifoResponse>), zx::Status> {
        let (peer_fifo, fifo) = zx::Fifo::create(16)?;
        Ok((
            Self {
                orchestrator,
                offset_map,
                max_transfer_blocks,
                block_size,
                peer_fifo,
                vmos: Mutex::default(),
            },
            fifo,
        ))
    }

    fn session_manager(&self) -> &SM {
        self.orchestrator.as_ref().borrow()
    }

    async fn handle_request(
        &self,
        request: fblock::SessionRequest,
    ) -> Result<HandleRequestResult, Error> {
        match request {
            fblock::SessionRequest::GetFifo { responder } => {
                let rights = zx::Rights::TRANSFER
                    | zx::Rights::READ
                    | zx::Rights::WRITE
                    | zx::Rights::SIGNAL
                    | zx::Rights::WAIT;
                match self.peer_fifo.duplicate_handle(rights) {
                    Ok(fifo) => responder.send(Ok(fifo.downcast()))?,
                    Err(s) => responder.send(Err(s.into_raw()))?,
                }
                Ok(HandleRequestResult::Ok)
            }
            fblock::SessionRequest::AttachVmo { vmo, responder } => {
                let info = vmo.info().map_err(Error::from)?;
                if info.flags.contains(zx::VmoInfoFlags::RESIZABLE) {
                    responder.send(Err(zx::Status::INVALID_ARGS.into_raw()))?;
                    return Ok(HandleRequestResult::Ok);
                }
                let size = info.size_bytes;
                let vmo = Arc::new(vmo);
                let vmo_id = {
                    let mut vmos = self.vmos.lock();
                    if vmos.len() == u16::MAX as usize {
                        responder.send(Err(zx::Status::NO_RESOURCES.into_raw()))?;
                        return Ok(HandleRequestResult::Ok);
                    } else {
                        let vmo_id = match vmos.last_entry() {
                            None => 1,
                            Some(o) => {
                                o.key().checked_add(1).unwrap_or_else(|| {
                                    let mut vmo_id = 1;
                                    // Find the first gap...
                                    for (&id, _) in &*vmos {
                                        if id > vmo_id {
                                            break;
                                        }
                                        vmo_id = id + 1;
                                    }
                                    vmo_id
                                })
                            }
                        };
                        vmos.insert(
                            vmo_id,
                            RegisteredVmo { vmo: vmo.clone(), size, mapping: None },
                        );
                        vmo_id
                    }
                };
                SM::on_attach_vmo(self.orchestrator.clone(), &vmo).await?;
                responder.send(Ok(&fblock::VmoId { id: vmo_id }))?;
                Ok(HandleRequestResult::Ok)
            }
            fblock::SessionRequest::Close { responder } => {
                Ok(HandleRequestResult::Closed(Box::new(move || {
                    if let Err(error) = responder.send(Ok(())) {
                        log::warn!(error:?; "Error sending close response");
                    }
                })))
            }
        }
    }

    /// Decodes `request`.
    fn decode_fifo_request(
        &self,
        session: SM::Session,
        request: &BlockFifoRequest,
    ) -> Result<DecodedRequest, Option<BlockFifoResponse>> {
        let flags = BlockIoFlag::from_bits_truncate(request.command.flags);

        let request_bytes = request.length as u64 * self.block_size as u64;

        let mut operation = BlockOpcode::from_primitive(request.command.opcode)
            .ok_or(zx::Status::INVALID_ARGS)
            .and_then(|code| {
                if flags.contains(BlockIoFlag::DECOMPRESS_WITH_ZSTD) {
                    if code != BlockOpcode::Read {
                        return Err(zx::Status::INVALID_ARGS);
                    }
                    if !SM::SUPPORTS_DECOMPRESSION {
                        return Err(zx::Status::NOT_SUPPORTED);
                    }
                }
                if matches!(code, BlockOpcode::Read | BlockOpcode::Write | BlockOpcode::Trim) {
                    if request.length == 0 {
                        return Err(zx::Status::INVALID_ARGS);
                    }
                    // Make sure the end offset won't wrap.
                    if request.dev_offset.checked_add(request.length as u64).is_none() {
                        return Err(zx::Status::OUT_OF_RANGE);
                    }
                }
                if matches!(code, BlockOpcode::Read | BlockOpcode::Write) {
                    let vmo_byte_offset = request
                        .vmo_offset
                        .checked_mul(self.block_size as u64)
                        .ok_or(zx::Status::OUT_OF_RANGE)?;
                    if request_bytes.checked_add(vmo_byte_offset).is_none() {
                        return Err(zx::Status::OUT_OF_RANGE);
                    }
                }
                Ok(match code {
                    BlockOpcode::Read => Operation::Read {
                        device_block_offset: request.dev_offset,
                        block_count: request.length,
                        _unused: 0,
                        vmo_offset: request
                            .vmo_offset
                            .checked_mul(self.block_size as u64)
                            .ok_or(zx::Status::OUT_OF_RANGE)?,
                        options: ReadOptions {
                            inline_crypto: InlineCryptoOptions {
                                is_enabled: flags.contains(BlockIoFlag::INLINE_ENCRYPTION_ENABLED),
                                dun: request.dun,
                                slot: request.slot,
                            },
                        },
                    },
                    BlockOpcode::Write => {
                        let mut options = WriteOptions {
                            inline_crypto: InlineCryptoOptions {
                                is_enabled: flags.contains(BlockIoFlag::INLINE_ENCRYPTION_ENABLED),
                                dun: request.dun,
                                slot: request.slot,
                            },
                            ..WriteOptions::default()
                        };
                        if flags.contains(BlockIoFlag::FORCE_ACCESS) {
                            options.flags |= WriteFlags::FORCE_ACCESS;
                        }
                        if flags.contains(BlockIoFlag::PRE_BARRIER) {
                            options.flags |= WriteFlags::PRE_BARRIER;
                        }
                        Operation::Write {
                            device_block_offset: request.dev_offset,
                            block_count: request.length,
                            _unused: 0,
                            options,
                            vmo_offset: request
                                .vmo_offset
                                .checked_mul(self.block_size as u64)
                                .ok_or(zx::Status::OUT_OF_RANGE)?,
                        }
                    }
                    BlockOpcode::Flush => Operation::Flush,
                    BlockOpcode::Trim => Operation::Trim {
                        device_block_offset: request.dev_offset,
                        block_count: request.length,
                    },
                    BlockOpcode::CloseVmo => Operation::CloseVmo,
                })
            });

        let group_or_request = if flags.contains(BlockIoFlag::GROUP_ITEM) {
            GroupOrRequest::Group(request.group)
        } else {
            GroupOrRequest::Request(request.reqid)
        };

        let mut active_requests = self.session_manager().active_requests().0.lock();
        let mut request_id = None;

        // Multiple Block I/O request may be sent as a group.
        // Notes:
        // - the group is identified by the group id in the request
        // - if using groups, a response will not be sent unless `BlockIoFlag::GROUP_LAST`
        //   flag is set.
        // - when processing a request of a group fails, subsequent requests of that
        //   group will not be processed.
        // - decompression is a special case, see block-fifo.h for semantics.
        //
        // Refer to sdk/fidl/fuchsia.hardware.block.driver/block.fidl for details.
        if group_or_request.is_group() {
            // Search for an existing entry that matches this group.  NOTE: This is a potentially
            // expensive way to find a group (it's iterating over all slots in the active-requests
            // slab).  This can be optimised easily should we need to.
            for (key, group) in &mut active_requests.requests {
                if group.group_or_request == group_or_request
                    && SM::session_eq(&group.session, &session)
                {
                    if group.req_id.is_some() {
                        // We have already received a request tagged as last.
                        if group.status.is_ok() {
                            group.status = Err(zx::Status::INVALID_ARGS);
                        }
                        // Ignore this request.
                        return Err(None);
                    }
                    // See if this is a continuation of a decompressed read.
                    if group.status.is_ok()
                        && let Some(info) = &mut group.decompression_info
                    {
                        if let Ok(Operation::Read {
                            device_block_offset,
                            mut block_count,
                            options,
                            vmo_offset: 0,
                            ..
                        }) = operation
                        {
                            let remaining_bytes = info
                                .compressed_range
                                .end
                                .next_multiple_of(self.block_size as usize)
                                as u64
                                - info.bytes_so_far;
                            if !flags.contains(BlockIoFlag::DECOMPRESS_WITH_ZSTD)
                                || request.total_compressed_bytes != 0
                                || request.uncompressed_bytes != 0
                                || request.compressed_prefix_bytes != 0
                                || (flags.contains(BlockIoFlag::GROUP_LAST)
                                    && info.bytes_so_far + request_bytes
                                        < info.compressed_range.end as u64)
                                || (!flags.contains(BlockIoFlag::GROUP_LAST)
                                    && request_bytes >= remaining_bytes)
                            {
                                group.status = Err(zx::Status::INVALID_ARGS);
                            } else {
                                // We are tolerant of `block_count` being more than we actually
                                // need.  This can happen if the client is working with a larger
                                // block size than the device block size.  For example, if Blobfs
                                // has a 8192 byte block size, but the device might has a 512 byte
                                // block size, it can ask for a multiple of 16 blocks, when fewer
                                // than that might actually be required to hold the compressed data.
                                // It is easier for us to tolerate this here than to get Blobfs to
                                // change to pass only the blocks that are required.
                                if request_bytes > remaining_bytes {
                                    block_count = (remaining_bytes / self.block_size as u64) as u32;
                                }

                                operation = Ok(Operation::ContinueDecompressedRead {
                                    offset: info.bytes_so_far,
                                    device_block_offset,
                                    block_count,
                                    options,
                                });

                                info.bytes_so_far += block_count as u64 * self.block_size as u64;
                            }
                        } else {
                            group.status = Err(zx::Status::INVALID_ARGS);
                        }
                    }
                    if flags.contains(BlockIoFlag::GROUP_LAST) {
                        group.req_id = Some(request.reqid);
                        // If the group has had an error, there is no point trying to issue this
                        // request.
                        if let Err(s) = group.status {
                            operation = Err(s);
                        }
                    } else if group.status.is_err() {
                        // The group has already encountered an error, so there is no point trying
                        // to issue this request.
                        return Err(None);
                    }
                    request_id = Some(RequestId(key));
                    group.count += 1;
                    break;
                }
            }
        }

        let is_single_request =
            !flags.contains(BlockIoFlag::GROUP_ITEM) || flags.contains(BlockIoFlag::GROUP_LAST);

        let mut decompression_info = None;
        let vmo = match operation {
            Ok(Operation::Read {
                device_block_offset,
                mut block_count,
                options,
                vmo_offset,
                ..
            }) => match self.vmos.lock().get_mut(&request.vmoid) {
                Some(registered_vmo) => {
                    if flags.contains(BlockIoFlag::DECOMPRESS_WITH_ZSTD) {
                        let compressed_range = request.compressed_prefix_bytes as usize
                            ..request.compressed_prefix_bytes as usize
                                + request.total_compressed_bytes as usize;
                        let required_buffer_size =
                            compressed_range.end.next_multiple_of(self.block_size as usize);

                        // Validate the initial decompression request.
                        if compressed_range.start >= compressed_range.end
                            || vmo_offset.checked_add(request.uncompressed_bytes as u64).is_none()
                            || (is_single_request && request_bytes < compressed_range.end as u64)
                            || (!is_single_request && request_bytes >= required_buffer_size as u64)
                        {
                            Err(zx::Status::INVALID_ARGS)
                        } else {
                            // We are tolerant of `block_count` being more than we actually need.
                            // This can happen if the client is working in a larger block size than
                            // the device block size.  For example, Blobfs has a 8192 byte block
                            // size, but the device might have a 512 byte block size.  It is easier
                            // for us to tolerate this here than to get Blobfs to change to pass
                            // only the blocks that are required.
                            let bytes_so_far = if request_bytes > required_buffer_size as u64 {
                                block_count =
                                    (required_buffer_size / self.block_size as usize) as u32;
                                required_buffer_size as u64
                            } else {
                                request_bytes
                            };

                            // To decompress, we need to have the target VMO mapped (cached).
                            registered_vmo
                                .get_or_create_mapping()
                                .and_then(|mapping| {
                                    // Make sure `vmo_offset` and `uncompressed_bytes` are
                                    // within range.
                                    if vmo_offset
                                        .checked_add(request.uncompressed_bytes as u64)
                                        .is_some_and(|end| end <= mapping.size as u64)
                                    {
                                        Ok(mapping)
                                    } else {
                                        Err(zx::Status::OUT_OF_RANGE)
                                    }
                                })
                                .map(|mapping| {
                                    // Convert the operation into a `StartDecompressedRead`
                                    // operation. For non-fragmented requests, this will be the only
                                    // operation, but if it's a fragmented read,
                                    // `ContinueDecompressedRead` operations will follow.
                                    operation = Ok(Operation::StartDecompressedRead {
                                        required_buffer_size,
                                        device_block_offset,
                                        block_count,
                                        options,
                                    });
                                    // Record sufficient information so that we can decompress when
                                    // all the requests complete.
                                    decompression_info = Some(DecompressionInfo {
                                        compressed_range,
                                        bytes_so_far,
                                        mapping,
                                        uncompressed_range: vmo_offset
                                            ..vmo_offset + request.uncompressed_bytes as u64,
                                        buffer: None,
                                    });
                                    None
                                })
                        }
                    } else {
                        registered_vmo
                            .validate_request(vmo_offset, request_bytes)
                            .map(|()| Some(registered_vmo.vmo.clone()))
                    }
                }
                None => Err(zx::Status::IO),
            },
            Ok(Operation::Write { vmo_offset, .. }) => {
                self.vmos.lock().get(&request.vmoid).map_or(Err(zx::Status::IO), |registered_vmo| {
                    registered_vmo
                        .validate_request(vmo_offset, request_bytes)
                        .map(|()| Some(registered_vmo.vmo.clone()))
                })
            }
            Ok(Operation::CloseVmo) => {
                self.vmos.lock().remove(&request.vmoid).map_or(
                    Err(zx::Status::IO),
                    |registered_vmo| {
                        let vmo_clone = registered_vmo.vmo.clone();
                        // Make sure the VMO is dropped after all current Epoch guards have been
                        // dropped.
                        Epoch::global().defer(move || drop(vmo_clone));
                        Ok(Some(registered_vmo.vmo))
                    },
                )
            }
            _ => Ok(None),
        }
        .unwrap_or_else(|e| {
            operation = Err(e);
            None
        });

        let trace_flow_id = NonZero::new(request.trace_flow_id);
        let request_id = request_id.unwrap_or_else(|| {
            RequestId(active_requests.requests.insert(ActiveRequest {
                session,
                group_or_request,
                trace_flow_id,
                _epoch_guard: Epoch::global().guard(),
                status: Ok(()),
                count: 1,
                req_id: is_single_request.then_some(request.reqid),
                decompression_info,
            }))
        });

        Ok(DecodedRequest {
            request_id,
            trace_flow_id,
            operation: operation.map_err(|status| {
                active_requests.complete_and_take_response(request_id, Err(status)).map(|(_, r)| r)
            })?,
            vmo,
        })
    }

    fn take_vmos(&self) -> BTreeMap<u16, RegisteredVmo> {
        std::mem::take(&mut *self.vmos.lock())
    }

    /// Maps the request and returns the mapped request with an optional remainder.
    fn map_request(
        &self,
        mut request: DecodedRequest,
        active_request: &mut ActiveRequest<SM::Session>,
    ) -> Result<(DecodedRequest, Option<DecodedRequest>), zx::Status> {
        if active_request.status.is_err() {
            return Err(zx::Status::BAD_STATE);
        }
        if let Some(blocks) = request.operation.blocks() {
            if !self.offset_map.are_blocks_within_source_range(blocks) {
                return Err(zx::Status::OUT_OF_RANGE);
            }
        }
        let remainder =
            request.operation.map(&self.offset_map, self.max_transfer_blocks, self.block_size)?;
        if remainder.is_some() {
            active_request.count += 1;
        }
        static CACHE: AtomicU64 = AtomicU64::new(0);
        if let Some(context) =
            fuchsia_trace::TraceCategoryContext::acquire_cached("storage", &CACHE)
        {
            use fuchsia_trace::ArgValue;
            let trace_args = [
                ArgValue::of("request_id", request.request_id.0),
                ArgValue::of("opcode", request.operation.trace_label()),
            ];
            let _scope =
                fuchsia_trace::duration("storage", "block_server::start_transaction", &trace_args);
            if let Some(trace_flow_id) = active_request.trace_flow_id {
                fuchsia_trace::flow_step(
                    &context,
                    "block_server::start_transaction",
                    trace_flow_id.get().into(),
                    &[],
                );
            }
        }
        let remainder = remainder.map(|operation| DecodedRequest { operation, ..request.clone() });
        Ok((request, remainder))
    }

    /// Drops all requests for which `pred` is true.
    ///
    /// NOTE: This should only be called once we are certain that the requests will not be
    /// completed asynchronously  Otherwise, requests might be completed twice.
    fn drop_active_requests(&self, pred: impl Fn(&SM::Session) -> bool) {
        self.session_manager().active_requests().0.lock().requests.retain(|_, r| !pred(&r.session));
    }

    /// Closes all grouped requests for which `pred` is true and which are held open pending the
    /// completion of their group.
    ///
    /// Normally, a request is dropped from ActiveRequests when it is completed.  However, if a
    /// request is part of a group, it will not be dropped until a request with GROUP_LAST arrives.
    /// If we're shutting down a session, the client may not ever send the GROUP_LAST, so we need to
    /// be sure to close these grouped requests.
    ///
    /// This is called during session shutdown in situations where [`Self::drop_active_requests`]
    /// cannot be used (e.g. for the callback interface, which hands off the responsibility of
    /// completing requests to its concrete implementation and cannot control when requests are
    /// completed relative to session shutdown).
    fn close_active_groups(&self, pred: impl Fn(&SM::Session) -> bool) {
        self.session_manager().active_requests().0.lock().requests.retain(|_, request| {
            if !pred(&request.session) || request.req_id.is_some() {
                return true;
            }
            // Mark the group as completed, and immediately drop any which have no outstanding
            // requests (since they will otherwise never be dropped).
            request.req_id = Some(u32::MAX);
            request.count > 0
        });
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct RequestId(usize);

#[derive(Clone, Debug)]
struct DecodedRequest {
    request_id: RequestId,
    trace_flow_id: TraceFlowId,
    operation: Operation,
    vmo: Option<Arc<zx::Vmo>>,
}

/// cbindgen:no-export
pub type WriteFlags = block_protocol::WriteFlags;
pub type WriteOptions = block_protocol::WriteOptions;
pub type ReadOptions = block_protocol::ReadOptions;
pub type InlineCryptoOptions = block_protocol::InlineCryptoOptions;

#[repr(C)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    // NOTE: On the C++ side, this ends up as a union and, for efficiency reasons, there is code
    // that assumes that some fields for reads and writes (and possibly trim) line-up (e.g. common
    // code can read `device_block_offset` from the read variant and then assume it's valid for the
    // write variant).
    Read {
        device_block_offset: u64,
        block_count: u32,
        _unused: u32,
        vmo_offset: u64,
        options: ReadOptions,
    },
    Write {
        device_block_offset: u64,
        block_count: u32,
        _unused: u32,
        vmo_offset: u64,
        options: WriteOptions,
    },
    Flush,
    Trim {
        device_block_offset: u64,
        block_count: u32,
    },
    /// This will never be seen by the C interface.
    CloseVmo,
    /// This will never be seen by the C interface.
    StartDecompressedRead {
        required_buffer_size: usize,
        device_block_offset: u64,
        block_count: u32,
        options: ReadOptions,
    },
    /// This will never be seen by the C interface.
    ContinueDecompressedRead {
        offset: u64,
        device_block_offset: u64,
        block_count: u32,
        options: ReadOptions,
    },
}

impl Operation {
    fn trace_label(&self) -> &'static str {
        match self {
            Operation::Read { .. } => "read",
            Operation::Write { .. } => "write",
            Operation::Flush { .. } => "flush",
            Operation::Trim { .. } => "trim",
            Operation::CloseVmo { .. } => "close_vmo",
            Operation::StartDecompressedRead { .. } => "start_decompressed_read",
            Operation::ContinueDecompressedRead { .. } => "continue_decompressed_read",
        }
    }

    /// Returns (offset, length).
    pub fn blocks(&self) -> Option<(u64, u32)> {
        match self {
            Operation::Read { device_block_offset, block_count, .. }
            | Operation::Write { device_block_offset, block_count, .. }
            | Operation::Trim { device_block_offset, block_count, .. } => {
                Some((*device_block_offset, *block_count))
            }
            _ => None,
        }
    }

    /// Returns mutable references to (offset, length).
    fn blocks_mut(&mut self) -> Option<(&mut u64, &mut u32)> {
        match self {
            Operation::Read { device_block_offset, block_count, .. }
            | Operation::Write { device_block_offset, block_count, .. }
            | Operation::Trim { device_block_offset, block_count, .. } => {
                Some((device_block_offset, block_count))
            }
            _ => None,
        }
    }

    /// Maps the operation using `offset_map` and returns the remainder if the request was split
    /// due to `max_transfer_blocks` or crossing mapping boundaries.
    fn map(
        &mut self,
        offset_map: &OffsetMap,
        max_transfer_blocks: Option<NonZero<u32>>,
        block_size: u32,
    ) -> Result<Option<Self>, zx::Status> {
        let mut max = match self {
            Operation::Read { .. } | Operation::Write { .. } => max_transfer_blocks.map(u32::from),
            _ => None,
        };
        let (offset, length) = match self.blocks_mut() {
            Some(b) => b,
            None => return Ok(None),
        };
        let orig_offset = *offset;
        if !offset_map.is_empty() {
            let (dev_offset, len) = offset_map.map(*offset).ok_or(zx::Status::OUT_OF_RANGE)?;
            *offset = dev_offset;
            max = match max {
                None => Some(len),
                Some(m) => Some(std::cmp::min(m, len)),
            };
        }
        if let Some(max) = max {
            if *length as u64 > max as u64 {
                let rem = *length - max;
                *length = max;
                return Ok(Some(match self {
                    Operation::Read {
                        device_block_offset: _,
                        block_count: _,
                        vmo_offset,
                        _unused,
                        options,
                    } => {
                        let mut options = *options;
                        options.inline_crypto.dun =
                            options.inline_crypto.dun.wrapping_add(max as u64);
                        Operation::Read {
                            device_block_offset: orig_offset + max as u64,
                            block_count: rem,
                            vmo_offset: *vmo_offset + max as u64 * block_size as u64,
                            _unused: *_unused,
                            options: options,
                        }
                    }
                    Operation::Write {
                        device_block_offset: _,
                        block_count: _,
                        _unused,
                        vmo_offset,
                        options,
                    } => {
                        let mut options = *options;
                        options.inline_crypto.dun =
                            options.inline_crypto.dun.wrapping_add(max as u64);
                        Operation::Write {
                            device_block_offset: orig_offset + max as u64,
                            block_count: rem,
                            _unused: *_unused,
                            vmo_offset: *vmo_offset + max as u64 * block_size as u64,
                            options: options,
                        }
                    }
                    Operation::Trim { device_block_offset: _, block_count: _ } => Operation::Trim {
                        device_block_offset: orig_offset + max as u64,
                        block_count: rem,
                    },
                    _ => unreachable!(),
                }));
            }
        }
        Ok(None)
    }

    /// Returns true if the specified write flags are set.
    pub fn has_write_flag(&self, value: WriteFlags) -> bool {
        if let Operation::Write { options, .. } = self {
            options.flags.contains(value)
        } else {
            false
        }
    }

    /// Removes `value` from the request's write flags and returns true if the flag was set.
    pub fn take_write_flag(&mut self, value: WriteFlags) -> bool {
        if let Operation::Write { options, .. } = self {
            let result = options.flags.contains(value);
            options.flags.remove(value);
            result
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum GroupOrRequest {
    Group(u16),
    Request(u32),
}

impl GroupOrRequest {
    fn is_group(&self) -> bool {
        matches!(self, Self::Group(_))
    }

    fn group_id(&self) -> Option<u16> {
        match self {
            Self::Group(id) => Some(*id),
            Self::Request(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockOffsetMapping, BlockServer, DeviceInfo, FIFO_MAX_REQUESTS, OffsetMap, Operation,
        PartitionInfo, TraceFlowId,
    };
    use assert_matches::assert_matches;
    use block_protocol::{
        BlockFifoCommand, BlockFifoRequest, BlockFifoResponse, InlineCryptoOptions, ReadOptions,
        WriteFlags, WriteOptions,
    };
    use fidl_fuchsia_storage_block as fblock;
    use fidl_fuchsia_storage_block::{BlockIoFlag, BlockOpcode};
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::FutureExt as _;
    use futures::channel::oneshot;
    use futures::future::BoxFuture;
    use std::borrow::Cow;
    use std::future::poll_fn;
    use std::num::NonZero;
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::task::{Context, Poll};

    #[derive(Default)]
    struct MockInterface {
        info: Option<DeviceInfo>,
        read_hook: Option<
            Box<
                dyn Fn(u64, u32, &Arc<zx::Vmo>, u64) -> BoxFuture<'static, Result<(), zx::Status>>
                    + Send
                    + Sync,
            >,
        >,
        write_hook:
            Option<Box<dyn Fn(u64) -> BoxFuture<'static, Result<(), zx::Status>> + Send + Sync>>,
        barrier_hook: Option<Box<dyn Fn() -> Result<(), zx::Status> + Send + Sync>>,
    }

    impl super::async_interface::Interface for MockInterface {
        async fn on_attach_vmo(&self, _vmo: &zx::Vmo) -> Result<(), zx::Status> {
            Ok(())
        }

        fn get_info(&self) -> Cow<'_, DeviceInfo> {
            match &self.info {
                Some(info) => Cow::Borrowed(info),
                None => Cow::Owned(test_device_info()),
            }
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
            if let Some(total) = self.get_info().block_count() {
                if device_block_offset >= total || total - device_block_offset < block_count as u64
                {
                    return Err(zx::Status::OUT_OF_RANGE);
                }
            }
            if let Some(read_hook) = &self.read_hook {
                read_hook(device_block_offset, block_count, vmo, vmo_offset).await
            } else {
                unimplemented!();
            }
        }

        async fn write(
            &self,
            device_block_offset: u64,
            _block_count: u32,
            _vmo: &Arc<zx::Vmo>,
            _vmo_offset: u64,
            opts: WriteOptions,
            _trace_flow_id: TraceFlowId,
        ) -> Result<(), zx::Status> {
            if opts.flags.contains(WriteFlags::PRE_BARRIER)
                && let Some(barrier_hook) = &self.barrier_hook
            {
                barrier_hook()?;
            }
            if let Some(write_hook) = &self.write_hook {
                write_hook(device_block_offset).await
            } else {
                unimplemented!();
            }
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
            unreachable!();
        }

        async fn get_volume_info(
            &self,
        ) -> Result<(fblock::VolumeManagerInfo, fblock::VolumeInfo), zx::Status> {
            // Hang forever for the test_requests_dont_block_sessions test.
            let () = std::future::pending().await;
            unreachable!();
        }
    }

    const BLOCK_SIZE: u32 = 512;
    const MAX_TRANSFER_BLOCKS: u32 = 10;

    fn test_device_info() -> DeviceInfo {
        DeviceInfo::Partition(PartitionInfo {
            device_flags: fblock::DeviceFlag::READONLY
                | fblock::DeviceFlag::BARRIER_SUPPORT
                | fblock::DeviceFlag::FUA_SUPPORT,
            max_transfer_blocks: NonZero::new(MAX_TRANSFER_BLOCKS),
            start_block_offset: Some(0),
            block_count: 100,
            type_guid: [1; 16],
            instance_guid: [2; 16],
            name: "foo".to_string(),
            flags: Some(0xabcd),
        })
    }

    #[fuchsia::test]
    async fn test_barriers_ordering() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();
        let vmo = zx::Vmo::create(2 * zx::system_get_page_size() as u64).unwrap();
        let barrier_called = Arc::new(AtomicBool::new(false));

        futures::join!(
            async move {
                let barrier_called_clone = barrier_called.clone();
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockInterface {
                        barrier_hook: Some(Box::new(move || {
                            barrier_called.store(true, Ordering::Relaxed);
                            Ok(())
                        })),
                        write_hook: Some(Box::new(move |device_block_offset| {
                            let barrier_called = barrier_called_clone.clone();
                            Box::pin(async move {
                                // The sleep allows the server to reorder the fifo requests.
                                if device_block_offset % 2 == 0 {
                                    fasync::Timer::new(fasync::MonotonicInstant::after(
                                        zx::MonotonicDuration::from_millis(200),
                                    ))
                                    .await;
                                }
                                assert!(barrier_called.load(Ordering::Relaxed));
                                Ok(())
                            })
                        })),
                        ..MockInterface::default()
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();
                assert_ne!(vmo_id.id, 0);

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Write.into_primitive(),
                            flags: BlockIoFlag::PRE_BARRIER.bits(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        dev_offset: 0,
                        length: 5,
                        vmo_offset: 6,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                for i in 0..10 {
                    writer
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Write.into_primitive(),
                                ..Default::default()
                            },
                            vmoid: vmo_id.id,
                            dev_offset: i + 1,
                            length: 5,
                            vmo_offset: 6,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
                for _ in 0..11 {
                    let mut response = BlockFifoResponse::default();
                    reader.read_entries(&mut response).await.unwrap();
                    assert_eq!(response.status, zx::sys::ZX_OK);
                }

                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_info() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        futures::join!(
            async {
                let block_server = BlockServer::new(BLOCK_SIZE, Arc::new(MockInterface::default()));
                block_server.handle_requests(stream).await.unwrap();
            },
            async {
                let expected_info = test_device_info();
                let partition_info = if let DeviceInfo::Partition(info) = &expected_info {
                    info
                } else {
                    unreachable!()
                };

                let block_info = proxy.get_info().await.unwrap().unwrap();
                assert_eq!(block_info.block_count, expected_info.block_count().unwrap());
                assert_eq!(
                    block_info.flags,
                    fblock::DeviceFlag::READONLY
                        | fblock::DeviceFlag::ZSTD_DECOMPRESSION_SUPPORT
                        | fblock::DeviceFlag::BARRIER_SUPPORT
                        | fblock::DeviceFlag::FUA_SUPPORT
                );

                assert_eq!(block_info.max_transfer_size, MAX_TRANSFER_BLOCKS * BLOCK_SIZE);

                let (status, type_guid) = proxy.get_type_guid().await.unwrap();
                assert_eq!(status, zx::sys::ZX_OK);
                assert_eq!(&type_guid.as_ref().unwrap().value, &partition_info.type_guid);

                let (status, instance_guid) = proxy.get_instance_guid().await.unwrap();
                assert_eq!(status, zx::sys::ZX_OK);
                assert_eq!(&instance_guid.as_ref().unwrap().value, &partition_info.instance_guid);

                let (status, name) = proxy.get_name().await.unwrap();
                assert_eq!(status, zx::sys::ZX_OK);
                assert_eq!(name.as_ref(), Some(&partition_info.name));

                let metadata = proxy.get_metadata().await.unwrap().expect("get_flags failed");
                assert_eq!(metadata.name, name);
                assert_eq!(metadata.type_guid.as_ref(), type_guid.as_deref());
                assert_eq!(metadata.instance_guid.as_ref(), instance_guid.as_deref());
                let expected_start = partition_info.start_block_offset.or(Some(0));
                assert_eq!(metadata.start_block_offset, expected_start);
                assert_eq!(metadata.num_blocks, Some(partition_info.block_count));
                assert_eq!(metadata.flags, partition_info.flags);

                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_attach_vmo() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
        let koid = vmo.koid().unwrap();

        futures::join!(
            async {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockInterface {
                        read_hook: Some(Box::new(move |_, _, vmo, _| {
                            assert_eq!(vmo.koid().unwrap(), koid);
                            Box::pin(async { Ok(()) })
                        })),
                        ..MockInterface::default()
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();
                assert_ne!(vmo_id.id, 0);

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                // Keep attaching VMOs until we eventually hit the maximum.
                let mut count = 1;
                loop {
                    match session_proxy
                        .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                        .await
                        .unwrap()
                    {
                        Ok(vmo_id) => assert_ne!(vmo_id.id, 0),
                        Err(e) => {
                            assert_eq!(e, zx::sys::ZX_ERR_NO_RESOURCES);
                            break;
                        }
                    }

                    // Only test every 10 to keep test time down.
                    if count % 10 == 0 {
                        writer
                            .write_entries(&BlockFifoRequest {
                                command: BlockFifoCommand {
                                    opcode: BlockOpcode::Read.into_primitive(),
                                    ..Default::default()
                                },
                                vmoid: vmo_id.id,
                                length: 1,
                                ..Default::default()
                            })
                            .await
                            .unwrap();

                        let mut response = BlockFifoResponse::default();
                        reader.read_entries(&mut response).await.unwrap();
                        assert_eq!(response.status, zx::sys::ZX_OK);
                    }

                    count += 1;
                }

                assert_eq!(count, u16::MAX as u64);

                // Detach the original VMO, and make sure we can then attach another one.
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::CloseVmo.into_primitive(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);

                let new_vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();
                // It should reuse the same ID.
                assert_eq!(new_vmo_id.id, vmo_id.id);

                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_attach_resizable_vmo_fails() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();
        let resizable_vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, 4096).unwrap();

        futures::join!(
            async {
                let block_server = BlockServer::new(BLOCK_SIZE, Arc::new(MockInterface::default()));
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();
                proxy.open_session(server).unwrap();
                let res = session_proxy.attach_vmo(resizable_vmo).await.unwrap();
                assert_eq!(res, Err(zx::sys::ZX_ERR_INVALID_ARGS));
                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_close() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let mut server = std::pin::pin!(
            async {
                let block_server = BlockServer::new(BLOCK_SIZE, Arc::new(MockInterface::default()));
                block_server.handle_requests(stream).await.unwrap();
            }
            .fuse()
        );

        let mut client = std::pin::pin!(
            async {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                // Dropping the proxy should not cause the session to terminate because the session
                // is still live.
                std::mem::drop(proxy);

                session_proxy.close().await.unwrap().unwrap();

                // Keep the session alive.  Calling `close` should cause the server to terminate.
                let _: () = std::future::pending().await;
            }
            .fuse()
        );

        futures::select!(
            _ = server => {}
            _ = client => unreachable!(),
        );
    }

    #[derive(Default)]
    struct IoMockInterface {
        do_checks: bool,
        expected_op: Arc<Mutex<Option<ExpectedOp>>>,
        return_errors: bool,
    }

    #[derive(Debug)]
    enum ExpectedOp {
        Read(u64, u32, u64),
        Write(u64, u32, u64),
        Trim(u64, u32),
        Flush,
    }

    impl super::async_interface::Interface for IoMockInterface {
        async fn on_attach_vmo(&self, _vmo: &zx::Vmo) -> Result<(), zx::Status> {
            Ok(())
        }

        fn get_info(&self) -> Cow<'_, DeviceInfo> {
            Cow::Owned(DeviceInfo::Block(crate::BlockInfo {
                block_count: 100,
                ..Default::default()
            }))
        }

        async fn read(
            &self,
            device_block_offset: u64,
            block_count: u32,
            _vmo: &Arc<zx::Vmo>,
            vmo_offset: u64,
            _opts: ReadOptions,
            _trace_flow_id: TraceFlowId,
        ) -> Result<(), zx::Status> {
            if self.return_errors {
                Err(zx::Status::INTERNAL)
            } else {
                if self.do_checks {
                    assert_matches!(
                        self.expected_op.lock().take(),
                        Some(ExpectedOp::Read(a, b, c)) if device_block_offset == a &&
                            block_count == b && vmo_offset / BLOCK_SIZE as u64 == c,
                        "Read {device_block_offset} {block_count} {vmo_offset}"
                    );
                }
                Ok(())
            }
        }

        async fn write(
            &self,
            device_block_offset: u64,
            block_count: u32,
            _vmo: &Arc<zx::Vmo>,
            vmo_offset: u64,
            _write_opts: WriteOptions,
            _trace_flow_id: TraceFlowId,
        ) -> Result<(), zx::Status> {
            if self.return_errors {
                Err(zx::Status::NOT_SUPPORTED)
            } else {
                if self.do_checks {
                    assert_matches!(
                        self.expected_op.lock().take(),
                        Some(ExpectedOp::Write(a, b, c)) if device_block_offset == a &&
                            block_count == b && vmo_offset / BLOCK_SIZE as u64 == c,
                        "Write {device_block_offset} {block_count} {vmo_offset}"
                    );
                }
                Ok(())
            }
        }

        async fn flush(&self, _trace_flow_id: TraceFlowId) -> Result<(), zx::Status> {
            if self.return_errors {
                Err(zx::Status::NO_RESOURCES)
            } else {
                if self.do_checks {
                    assert_matches!(self.expected_op.lock().take(), Some(ExpectedOp::Flush));
                }
                Ok(())
            }
        }

        async fn trim(
            &self,
            device_block_offset: u64,
            block_count: u32,
            _trace_flow_id: TraceFlowId,
        ) -> Result<(), zx::Status> {
            if self.return_errors {
                Err(zx::Status::NO_MEMORY)
            } else {
                if self.do_checks {
                    assert_matches!(
                        self.expected_op.lock().take(),
                        Some(ExpectedOp::Trim(a, b)) if device_block_offset == a &&
                            block_count == b,
                        "Trim {device_block_offset} {block_count}"
                    );
                }
                Ok(())
            }
        }
    }

    #[fuchsia::test]
    async fn test_io() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let expected_op = Arc::new(Mutex::new(None));
        let expected_op_clone = expected_op.clone();

        let server = async {
            let block_server = BlockServer::new(
                BLOCK_SIZE,
                Arc::new(IoMockInterface {
                    return_errors: false,
                    do_checks: true,
                    expected_op: expected_op_clone,
                }),
            );
            block_server.handle_requests(stream).await.unwrap();
        };

        let client = async move {
            let (session_proxy, server) = fidl::endpoints::create_proxy();

            proxy.open_session(server).unwrap();

            let vmo = zx::Vmo::create(2 * zx::system_get_page_size() as u64).unwrap();
            let vmo_id = session_proxy
                .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                .await
                .unwrap()
                .unwrap();

            let mut fifo =
                fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
            let (mut reader, mut writer) = fifo.async_io();

            // READ
            *expected_op.lock() = Some(ExpectedOp::Read(1, 2, 3));
            writer
                .write_entries(&BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Read.into_primitive(),
                        ..Default::default()
                    },
                    vmoid: vmo_id.id,
                    dev_offset: 1,
                    length: 2,
                    vmo_offset: 3,
                    ..Default::default()
                })
                .await
                .unwrap();

            let mut response = BlockFifoResponse::default();
            reader.read_entries(&mut response).await.unwrap();
            assert_eq!(response.status, zx::sys::ZX_OK);

            // WRITE
            *expected_op.lock() = Some(ExpectedOp::Write(4, 5, 6));
            writer
                .write_entries(&BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Write.into_primitive(),
                        ..Default::default()
                    },
                    vmoid: vmo_id.id,
                    dev_offset: 4,
                    length: 5,
                    vmo_offset: 6,
                    ..Default::default()
                })
                .await
                .unwrap();

            let mut response = BlockFifoResponse::default();
            reader.read_entries(&mut response).await.unwrap();
            assert_eq!(response.status, zx::sys::ZX_OK);

            // FLUSH
            *expected_op.lock() = Some(ExpectedOp::Flush);
            writer
                .write_entries(&BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Flush.into_primitive(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .await
                .unwrap();

            reader.read_entries(&mut response).await.unwrap();
            assert_eq!(response.status, zx::sys::ZX_OK);

            // TRIM
            *expected_op.lock() = Some(ExpectedOp::Trim(7, 8));
            writer
                .write_entries(&BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Trim.into_primitive(),
                        ..Default::default()
                    },
                    dev_offset: 7,
                    length: 8,
                    ..Default::default()
                })
                .await
                .unwrap();

            reader.read_entries(&mut response).await.unwrap();
            assert_eq!(response.status, zx::sys::ZX_OK);

            std::mem::drop(proxy);
        };

        futures::join!(server, client);
    }

    #[fuchsia::test]
    async fn test_io_errors() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        futures::join!(
            async {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(IoMockInterface {
                        return_errors: true,
                        do_checks: false,
                        expected_op: Arc::new(Mutex::new(None)),
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                // READ
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        length: 1,
                        reqid: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_INTERNAL);

                // WRITE
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Write.into_primitive(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        length: 1,
                        reqid: 2,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_NOT_SUPPORTED);

                // FLUSH
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Flush.into_primitive(),
                            ..Default::default()
                        },
                        reqid: 3,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_NO_RESOURCES);

                // TRIM
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Trim.into_primitive(),
                            ..Default::default()
                        },
                        reqid: 4,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_NO_MEMORY);

                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_invalid_args() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        futures::join!(
            async {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(IoMockInterface {
                        return_errors: false,
                        do_checks: false,
                        expected_op: Arc::new(Mutex::new(None)),
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());

                async fn test(
                    fifo: &mut fasync::Fifo<BlockFifoResponse, BlockFifoRequest>,
                    request: BlockFifoRequest,
                ) -> Result<(), zx::Status> {
                    let (mut reader, mut writer) = fifo.async_io();
                    writer.write_entries(&request).await.unwrap();
                    let mut response = BlockFifoResponse::default();
                    reader.read_entries(&mut response).await.unwrap();
                    zx::Status::ok(response.status)
                }

                // READ

                let good_read_request = || BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Read.into_primitive(),
                        ..Default::default()
                    },
                    length: 1,
                    vmoid: vmo_id.id,
                    ..Default::default()
                };

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest { vmoid: vmo_id.id + 1, ..good_read_request() }
                    )
                    .await,
                    Err(zx::Status::IO)
                );

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest {
                            vmo_offset: 0xffff_ffff_ffff_ffff,
                            ..good_read_request()
                        }
                    )
                    .await,
                    Err(zx::Status::OUT_OF_RANGE)
                );

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest {
                            vmo_offset: 0x007f_ffff_ffff_ffff,
                            length: 2,
                            ..good_read_request()
                        }
                    )
                    .await,
                    Err(zx::Status::OUT_OF_RANGE)
                );

                assert_eq!(
                    test(&mut fifo, BlockFifoRequest { length: 0, ..good_read_request() }).await,
                    Err(zx::Status::INVALID_ARGS)
                );

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest { vmo_offset: 8, length: 1, ..good_read_request() }
                    )
                    .await,
                    Err(zx::Status::OUT_OF_RANGE)
                );

                // WRITE

                let good_write_request = || BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Write.into_primitive(),
                        ..Default::default()
                    },
                    length: 1,
                    vmoid: vmo_id.id,
                    ..Default::default()
                };

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest { vmoid: vmo_id.id + 1, ..good_write_request() }
                    )
                    .await,
                    Err(zx::Status::IO)
                );

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest {
                            vmo_offset: 0xffff_ffff_ffff_ffff,
                            ..good_write_request()
                        }
                    )
                    .await,
                    Err(zx::Status::OUT_OF_RANGE)
                );

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest {
                            vmo_offset: 0x007f_ffff_ffff_ffff,
                            length: 2,
                            ..good_write_request()
                        }
                    )
                    .await,
                    Err(zx::Status::OUT_OF_RANGE)
                );

                assert_eq!(
                    test(&mut fifo, BlockFifoRequest { length: 0, ..good_write_request() }).await,
                    Err(zx::Status::INVALID_ARGS)
                );

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest { vmo_offset: 8, length: 1, ..good_write_request() }
                    )
                    .await,
                    Err(zx::Status::OUT_OF_RANGE)
                );

                // CLOSE VMO

                assert_eq!(
                    test(
                        &mut fifo,
                        BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::CloseVmo.into_primitive(),
                                ..Default::default()
                            },
                            vmoid: vmo_id.id + 1,
                            ..Default::default()
                        }
                    )
                    .await,
                    Err(zx::Status::IO)
                );

                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_concurrent_requests() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let waiting_readers = Arc::new(Mutex::new(Vec::new()));
        let waiting_readers_clone = waiting_readers.clone();

        futures::join!(
            async move {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockInterface {
                        read_hook: Some(Box::new(move |dev_block_offset, _, _, _| {
                            let (tx, rx) = oneshot::channel();
                            waiting_readers_clone.lock().push((dev_block_offset as u32, tx));
                            Box::pin(async move {
                                let _ = rx.await;
                                Ok(())
                            })
                        })),
                        ..MockInterface::default()
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            ..Default::default()
                        },
                        reqid: 1,
                        dev_offset: 1, // Intentionally use the same as `reqid`.
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            ..Default::default()
                        },
                        reqid: 2,
                        dev_offset: 2,
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                // Wait till both those entries are pending.
                poll_fn(|cx: &mut Context<'_>| {
                    if waiting_readers.lock().len() == 2 {
                        Poll::Ready(())
                    } else {
                        // Yield to the executor.
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;

                let mut response = BlockFifoResponse::default();
                assert!(futures::poll!(pin!(reader.read_entries(&mut response))).is_pending());

                let (id, tx) = waiting_readers.lock().pop().unwrap();
                tx.send(()).unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);
                assert_eq!(response.reqid, id);

                assert!(futures::poll!(pin!(reader.read_entries(&mut response))).is_pending());

                let (id, tx) = waiting_readers.lock().pop().unwrap();
                tx.send(()).unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);
                assert_eq!(response.reqid, id);
            }
        );
    }

    #[fuchsia::test]
    async fn test_session_close_is_synchronous() {
        use futures::{FutureExt as _, StreamExt as _};

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let (start_tx, mut start_rx) = futures::channel::mpsc::channel(1);
        let (finish_tx, finish_rx) = futures::channel::oneshot::channel();
        let finish_rx = Arc::new(Mutex::new(Some(finish_rx)));

        futures::join!(
            async move {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockInterface {
                        read_hook: Some(Box::new(move |_, _, _, _| {
                            let mut start_tx = start_tx.clone();
                            let finish_rx = finish_rx.lock().take().unwrap();
                            Box::pin(async move {
                                start_tx.try_send(()).unwrap();
                                let _ = finish_rx.await;
                                Ok(())
                            })
                        })),
                        ..MockInterface::default()
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();
                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo = fasync::Fifo::<BlockFifoResponse, BlockFifoRequest>::from_fifo(
                    session_proxy.get_fifo().await.unwrap().unwrap(),
                );
                let (_reader, mut writer) = fifo.async_io();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            ..Default::default()
                        },
                        reqid: 1,
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                // Wait for the read to actually start.
                start_rx.next().await.unwrap();

                // The close request shouldn't complete yet because the read is still hanging.
                let mut close_fut = std::pin::pin!(session_proxy.close().fuse());
                let mut timer_fut = std::pin::pin!(
                    fasync::Timer::new(std::time::Duration::from_millis(100)).fuse()
                );
                futures::select! {
                    res = close_fut => panic!("close completed too early: {:?}", res),
                    _ = timer_fut => {}
                }

                // Finish the pending request.
                finish_tx.send(()).unwrap();

                // Verify that close() now completes.
                close_fut.await.unwrap().unwrap();

                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_groups() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        futures::join!(
            async move {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockInterface {
                        read_hook: Some(Box::new(move |_, _, _, _| Box::pin(async { Ok(()) }))),
                        ..MockInterface::default()
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            flags: BlockIoFlag::GROUP_ITEM.bits(),
                            ..Default::default()
                        },
                        group: 1,
                        vmoid: vmo_id.id,
                        length: 1,
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
                        reqid: 2,
                        group: 1,
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);
                assert_eq!(response.reqid, 2);
                assert_eq!(response.group, 1);
            }
        );
    }

    #[fuchsia::test]
    async fn test_group_error() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        futures::join!(
            async move {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockInterface {
                        read_hook: Some(Box::new(move |_, _, _, _| {
                            counter_clone.fetch_add(1, Ordering::Relaxed);
                            Box::pin(async { Err(zx::Status::BAD_STATE) })
                        })),
                        ..MockInterface::default()
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            flags: BlockIoFlag::GROUP_ITEM.bits(),
                            ..Default::default()
                        },
                        group: 1,
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                // Wait until processed.
                poll_fn(|cx: &mut Context<'_>| {
                    if counter.load(Ordering::Relaxed) == 1 {
                        Poll::Ready(())
                    } else {
                        // Yield to the executor.
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;

                let mut response = BlockFifoResponse::default();
                assert!(futures::poll!(pin!(reader.read_entries(&mut response))).is_pending());

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            flags: BlockIoFlag::GROUP_ITEM.bits(),
                            ..Default::default()
                        },
                        group: 1,
                        vmoid: vmo_id.id,
                        length: 1,
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
                        reqid: 2,
                        group: 1,
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_BAD_STATE);
                assert_eq!(response.reqid, 2);
                assert_eq!(response.group, 1);

                assert!(futures::poll!(pin!(reader.read_entries(&mut response))).is_pending());

                // Only the first request should have been processed.
                assert_eq!(counter.load(Ordering::Relaxed), 1);
            }
        );
    }

    #[fuchsia::test]
    async fn test_group_with_two_lasts() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let (tx, rx) = oneshot::channel();

        futures::join!(
            async move {
                let rx = Mutex::new(Some(rx));
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockInterface {
                        read_hook: Some(Box::new(move |_, _, _, _| {
                            let rx = rx.lock().take().unwrap();
                            Box::pin(async {
                                let _ = rx.await;
                                Ok(())
                            })
                        })),
                        ..MockInterface::default()
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                            ..Default::default()
                        },
                        reqid: 1,
                        group: 1,
                        vmoid: vmo_id.id,
                        length: 1,
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
                        reqid: 2,
                        group: 1,
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                // Send an independent request to flush through the fifo.
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::CloseVmo.into_primitive(),
                            ..Default::default()
                        },
                        reqid: 3,
                        vmoid: vmo_id.id,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                // It should succeed.
                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);
                assert_eq!(response.reqid, 3);

                // Now release the original request.
                tx.send(()).unwrap();

                // The response should be for the first message tagged as last, and it should be
                // an error because we sent two messages with the LAST marker.
                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_INVALID_ARGS);
                assert_eq!(response.reqid, 1);
                assert_eq!(response.group, 1);
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_requests_dont_block_sessions() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let (tx, rx) = oneshot::channel();

        fasync::Task::local(async move {
            let rx = Mutex::new(Some(rx));
            let block_server = BlockServer::new(
                BLOCK_SIZE,
                Arc::new(MockInterface {
                    read_hook: Some(Box::new(move |_, _, _, _| {
                        let rx = rx.lock().take().unwrap();
                        Box::pin(async {
                            let _ = rx.await;
                            Ok(())
                        })
                    })),
                    ..MockInterface::default()
                }),
            );
            block_server.handle_requests(stream).await.unwrap();
        })
        .detach();

        let mut fut = pin!(async {
            let (session_proxy, server) = fidl::endpoints::create_proxy();

            proxy.open_session(server).unwrap();

            let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
            let vmo_id = session_proxy
                .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                .await
                .unwrap()
                .unwrap();

            let mut fifo =
                fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
            let (mut reader, mut writer) = fifo.async_io();

            writer
                .write_entries(&BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Read.into_primitive(),
                        flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                        ..Default::default()
                    },
                    reqid: 1,
                    group: 1,
                    vmoid: vmo_id.id,
                    length: 1,
                    ..Default::default()
                })
                .await
                .unwrap();

            let mut response = BlockFifoResponse::default();
            reader.read_entries(&mut response).await.unwrap();
            assert_eq!(response.status, zx::sys::ZX_OK);
        });

        // The response won't come back until we send on `tx`.
        assert!(fasync::TestExecutor::poll_until_stalled(&mut fut).await.is_pending());

        let mut fut2 = pin!(proxy.get_volume_info());

        // get_volume_info is set up to stall forever.
        assert!(fasync::TestExecutor::poll_until_stalled(&mut fut2).await.is_pending());

        // If we now free up the first future, it should resolve; the stalled call to
        // get_volume_info should not block the fifo response.
        let _ = tx.send(());

        assert!(fasync::TestExecutor::poll_until_stalled(&mut fut).await.is_ready());
    }

    #[fuchsia::test]
    async fn test_request_flow_control() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        // The client will ensure that MAX_REQUESTS are queued up before firing `event`, and the
        // server will block until that happens.
        const MAX_REQUESTS: u64 = FIFO_MAX_REQUESTS as u64;
        let event = Arc::new((event_listener::Event::new(), AtomicBool::new(false)));
        let event_clone = event.clone();
        futures::join!(
            async move {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockInterface {
                        info: Some(DeviceInfo::Partition(PartitionInfo {
                            block_count: 1000,
                            ..Default::default()
                        })),
                        read_hook: Some(Box::new(move |_, _, _, _| {
                            let event_clone = event_clone.clone();
                            Box::pin(async move {
                                if !event_clone.1.load(Ordering::SeqCst) {
                                    event_clone.0.listen().await;
                                }
                                Ok(())
                            })
                        })),
                        ..MockInterface::default()
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                for i in 0..MAX_REQUESTS {
                    writer
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Read.into_primitive(),
                                ..Default::default()
                            },
                            reqid: (i + 1) as u32,
                            dev_offset: i,
                            vmoid: vmo_id.id,
                            length: 1,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
                assert!(
                    futures::poll!(pin!(writer.write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            ..Default::default()
                        },
                        reqid: u32::MAX,
                        dev_offset: MAX_REQUESTS,
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })))
                    .is_pending()
                );
                // OK, let the server start to process.
                event.1.store(true, Ordering::SeqCst);
                event.0.notify(usize::MAX);
                // For each entry we read, make sure we can write a new one in.
                let mut finished_reqids = vec![];
                for i in MAX_REQUESTS..2 * MAX_REQUESTS {
                    let mut response = BlockFifoResponse::default();
                    reader.read_entries(&mut response).await.unwrap();
                    assert_eq!(response.status, zx::sys::ZX_OK);
                    finished_reqids.push(response.reqid);
                    writer
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Read.into_primitive(),
                                ..Default::default()
                            },
                            reqid: (i + 1) as u32,
                            dev_offset: i,
                            vmoid: vmo_id.id,
                            length: 1,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
                let mut response = BlockFifoResponse::default();
                for _ in 0..MAX_REQUESTS {
                    reader.read_entries(&mut response).await.unwrap();
                    assert_eq!(response.status, zx::sys::ZX_OK);
                    finished_reqids.push(response.reqid);
                }
                // Verify that we got a response for each request.  Note that we can't assume FIFO
                // ordering.
                finished_reqids.sort();
                assert_eq!(finished_reqids.len(), 2 * MAX_REQUESTS as usize);
                let mut i = 1;
                for reqid in finished_reqids {
                    assert_eq!(reqid, i);
                    i += 1;
                }
            }
        );
    }

    #[fuchsia::test]
    async fn test_passthrough_io_with_fixed_map() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let expected_op = Arc::new(Mutex::new(None));
        let expected_op_clone = expected_op.clone();
        futures::join!(
            async {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(IoMockInterface {
                        return_errors: false,
                        do_checks: true,
                        expected_op: expected_op_clone,
                    }),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();

                let mapping = fblock::BlockOffsetMapping { target_block_offset: 10, length: 20 };
                proxy.open_session_with_options(server, &[mapping]).unwrap();

                let vmo = zx::Vmo::create(2 * zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                // READ
                *expected_op.lock() = Some(ExpectedOp::Read(11, 2, 3));
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        dev_offset: 1,
                        length: 2,
                        vmo_offset: 3,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);

                // WRITE
                *expected_op.lock() = Some(ExpectedOp::Write(14, 5, 6));
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Write.into_primitive(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        dev_offset: 4,
                        length: 5,
                        vmo_offset: 6,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);

                // FLUSH
                *expected_op.lock() = Some(ExpectedOp::Flush);
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Flush.into_primitive(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);

                // TRIM
                *expected_op.lock() = Some(ExpectedOp::Trim(17, 3));
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Trim.into_primitive(),
                            ..Default::default()
                        },
                        dev_offset: 7,
                        length: 3,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_OK);

                // READ past window
                *expected_op.lock() = None;
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        dev_offset: 19,
                        length: 2,
                        vmo_offset: 3,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_OUT_OF_RANGE);

                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    fn operation_map() {
        const BLOCK_SIZE: u32 = 512;

        #[track_caller]
        fn expect_map_result(
            mut operation: Operation,
            mapping: Option<fblock::BlockOffsetMapping>,
            max_blocks: Option<NonZero<u32>>,
            expected_operations: Vec<Operation>,
        ) {
            let offset_map = mapping
                .map(|m| {
                    let map: BlockOffsetMapping = (&m).try_into().unwrap();
                    OffsetMap::new(vec![map]).unwrap()
                })
                .unwrap_or_else(OffsetMap::empty);
            let mut ops = vec![];
            while let Some(remainder) = operation.map(&offset_map, max_blocks, BLOCK_SIZE).unwrap()
            {
                ops.push(operation);
                operation = remainder;
            }
            ops.push(operation);
            assert_eq!(ops, expected_operations);
        }

        // No limits
        expect_map_result(
            Operation::Read {
                device_block_offset: 10,
                block_count: 200,
                _unused: 0,
                vmo_offset: 0,
                options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
            },
            None,
            None,
            vec![Operation::Read {
                device_block_offset: 10,
                block_count: 200,
                _unused: 0,
                vmo_offset: 0,
                options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
            }],
        );

        // Max block count
        expect_map_result(
            Operation::Read {
                device_block_offset: 10,
                block_count: 200,
                _unused: 0,
                vmo_offset: 0,
                options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
            },
            None,
            NonZero::new(120),
            vec![
                Operation::Read {
                    device_block_offset: 10,
                    block_count: 120,
                    _unused: 0,
                    vmo_offset: 0,
                    options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
                },
                Operation::Read {
                    device_block_offset: 130,
                    block_count: 80,
                    _unused: 0,
                    vmo_offset: 120 * BLOCK_SIZE as u64,
                    options: ReadOptions {
                        // The DUN should be offset by the number of blocks in the first request.
                        inline_crypto: InlineCryptoOptions::enabled(1, 1000 + 120),
                    },
                },
            ],
        );
        expect_map_result(
            Operation::Trim { device_block_offset: 10, block_count: 200 },
            None,
            NonZero::new(120),
            vec![Operation::Trim { device_block_offset: 10, block_count: 200 }],
        );

        // Remapping + Max block count
        expect_map_result(
            Operation::Read {
                device_block_offset: 0,
                block_count: 200,
                _unused: 0,
                vmo_offset: 0,
                options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
            },
            Some(fblock::BlockOffsetMapping { target_block_offset: 100, length: 200 }),
            NonZero::new(120),
            vec![
                Operation::Read {
                    device_block_offset: 100,
                    block_count: 120,
                    _unused: 0,
                    vmo_offset: 0,
                    options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
                },
                Operation::Read {
                    device_block_offset: 220,
                    block_count: 80,
                    _unused: 0,
                    vmo_offset: 120 * BLOCK_SIZE as u64,
                    options: ReadOptions {
                        inline_crypto: InlineCryptoOptions::enabled(1, 1000 + 120),
                    },
                },
            ],
        );
        expect_map_result(
            Operation::Trim { device_block_offset: 0, block_count: 200 },
            Some(fblock::BlockOffsetMapping { target_block_offset: 100, length: 200 }),
            NonZero::new(120),
            vec![Operation::Trim { device_block_offset: 100, block_count: 200 }],
        );

        // Multi-extent remapping
        let multi_extent_map = OffsetMap::new(vec![
            BlockOffsetMapping { target_block_offset: 100, length: 10 },
            BlockOffsetMapping { target_block_offset: 200, length: 20 },
        ])
        .unwrap();

        fn expect_multi_map_result(
            mut operation: Operation,
            offset_map: &OffsetMap,
            max_blocks: Option<NonZero<u32>>,
            expected_operations: Vec<Operation>,
        ) {
            let mut ops = vec![];
            while let Some(remainder) = operation.map(offset_map, max_blocks, BLOCK_SIZE).unwrap() {
                ops.push(operation);
                operation = remainder;
            }
            ops.push(operation);
            assert_eq!(ops, expected_operations);
        }

        // Read spanning multi-extents (logical offset 5, count 15 -> 5 blocks in extent 0, 10 in
        // extent 1)
        expect_multi_map_result(
            Operation::Read {
                device_block_offset: 5,
                block_count: 15,
                _unused: 0,
                vmo_offset: 0,
                options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
            },
            &multi_extent_map,
            None,
            vec![
                Operation::Read {
                    device_block_offset: 105,
                    block_count: 5,
                    _unused: 0,
                    vmo_offset: 0,
                    options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
                },
                Operation::Read {
                    device_block_offset: 200,
                    block_count: 10,
                    _unused: 0,
                    vmo_offset: 5 * BLOCK_SIZE as u64,
                    options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1005) },
                },
            ],
        );

        // Read spanning multi-extents with max_transfer_blocks limit
        expect_multi_map_result(
            Operation::Read {
                device_block_offset: 5,
                block_count: 15,
                _unused: 0,
                vmo_offset: 0,
                options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
            },
            &multi_extent_map,
            NonZero::new(7),
            vec![
                Operation::Read {
                    device_block_offset: 105,
                    block_count: 5,
                    _unused: 0,
                    vmo_offset: 0,
                    options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1000) },
                },
                Operation::Read {
                    device_block_offset: 200,
                    block_count: 7,
                    _unused: 0,
                    vmo_offset: 5 * BLOCK_SIZE as u64,
                    options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1005) },
                },
                Operation::Read {
                    device_block_offset: 207,
                    block_count: 3,
                    _unused: 0,
                    vmo_offset: 12 * BLOCK_SIZE as u64,
                    options: ReadOptions { inline_crypto: InlineCryptoOptions::enabled(1, 1012) },
                },
            ],
        );

        // Write spanning multi-extents
        expect_multi_map_result(
            Operation::Write {
                device_block_offset: 5,
                block_count: 15,
                _unused: 0,
                vmo_offset: 0,
                options: WriteOptions {
                    inline_crypto: InlineCryptoOptions::enabled(1, 2000),
                    flags: WriteFlags::empty(),
                },
            },
            &multi_extent_map,
            None,
            vec![
                Operation::Write {
                    device_block_offset: 105,
                    block_count: 5,
                    _unused: 0,
                    vmo_offset: 0,
                    options: WriteOptions {
                        inline_crypto: InlineCryptoOptions::enabled(1, 2000),
                        flags: WriteFlags::empty(),
                    },
                },
                Operation::Write {
                    device_block_offset: 200,
                    block_count: 10,
                    _unused: 0,
                    vmo_offset: 5 * BLOCK_SIZE as u64,
                    options: WriteOptions {
                        inline_crypto: InlineCryptoOptions::enabled(1, 2005),
                        flags: WriteFlags::empty(),
                    },
                },
            ],
        );

        // Trim spanning multi-extents
        expect_multi_map_result(
            Operation::Trim { device_block_offset: 5, block_count: 15 },
            &multi_extent_map,
            None,
            vec![
                Operation::Trim { device_block_offset: 105, block_count: 5 },
                Operation::Trim { device_block_offset: 200, block_count: 10 },
            ],
        );

        // Large extent test (length > u32::MAX)
        let large_extent_map = OffsetMap::new(vec![BlockOffsetMapping {
            target_block_offset: 100,
            length: (u32::MAX as u64) + 10,
        }])
        .unwrap();

        assert_eq!(large_extent_map.map(0), Some((100, u32::MAX)));
    }

    // Verifies that if the pre-flush (for a simulated barrier) fails, the write is not executed.
    #[fuchsia::test]
    async fn test_pre_barrier_flush_failure() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        struct NoBarrierInterface;
        impl super::async_interface::Interface for NoBarrierInterface {
            fn get_info(&self) -> Cow<'_, DeviceInfo> {
                Cow::Owned(DeviceInfo::Partition(PartitionInfo {
                    device_flags: fblock::DeviceFlag::empty(), // No BARRIER_SUPPORT
                    max_transfer_blocks: NonZero::new(100),
                    start_block_offset: Some(0),
                    block_count: 100,
                    type_guid: [0; 16],
                    instance_guid: [0; 16],
                    name: "test".to_string(),
                    flags: Some(0),
                }))
            }
            async fn on_attach_vmo(&self, _vmo: &zx::Vmo) -> Result<(), zx::Status> {
                Ok(())
            }
            async fn read(
                &self,
                _: u64,
                _: u32,
                _: &Arc<zx::Vmo>,
                _: u64,
                _: ReadOptions,
                _: TraceFlowId,
            ) -> Result<(), zx::Status> {
                unreachable!()
            }
            async fn write(
                &self,
                _: u64,
                _: u32,
                _: &Arc<zx::Vmo>,
                _: u64,
                _: WriteOptions,
                _: TraceFlowId,
            ) -> Result<(), zx::Status> {
                panic!("Write should not be called");
            }
            async fn flush(&self, _: TraceFlowId) -> Result<(), zx::Status> {
                Err(zx::Status::IO)
            }
            async fn trim(&self, _: u64, _: u32, _: TraceFlowId) -> Result<(), zx::Status> {
                unreachable!()
            }
        }

        futures::join!(
            async move {
                let block_server = BlockServer::new(BLOCK_SIZE, Arc::new(NoBarrierInterface));
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();
                proxy.open_session(server).unwrap();
                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Write.into_primitive(),
                            flags: BlockIoFlag::PRE_BARRIER.bits(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_IO);
            }
        );
    }

    // Verifies that if the write fails when a post-flush is required (for a simulated FUA), the
    // post-flush is not executed.
    #[fuchsia::test]
    async fn test_post_barrier_write_failure() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        struct NoBarrierInterface;
        impl super::async_interface::Interface for NoBarrierInterface {
            fn get_info(&self) -> Cow<'_, DeviceInfo> {
                Cow::Owned(DeviceInfo::Partition(PartitionInfo {
                    device_flags: fblock::DeviceFlag::empty(), // No FUA_SUPPORT
                    max_transfer_blocks: NonZero::new(100),
                    start_block_offset: Some(0),
                    block_count: 100,
                    type_guid: [0; 16],
                    instance_guid: [0; 16],
                    name: "test".to_string(),
                    flags: Some(0),
                }))
            }
            async fn on_attach_vmo(&self, _vmo: &zx::Vmo) -> Result<(), zx::Status> {
                Ok(())
            }
            async fn read(
                &self,
                _: u64,
                _: u32,
                _: &Arc<zx::Vmo>,
                _: u64,
                _: ReadOptions,
                _: TraceFlowId,
            ) -> Result<(), zx::Status> {
                unreachable!()
            }
            async fn write(
                &self,
                _: u64,
                _: u32,
                _: &Arc<zx::Vmo>,
                _: u64,
                _: WriteOptions,
                _: TraceFlowId,
            ) -> Result<(), zx::Status> {
                Err(zx::Status::IO)
            }
            async fn flush(&self, _: TraceFlowId) -> Result<(), zx::Status> {
                panic!("Flush should not be called")
            }
            async fn trim(&self, _: u64, _: u32, _: TraceFlowId) -> Result<(), zx::Status> {
                unreachable!()
            }
        }

        futures::join!(
            async move {
                let block_server = BlockServer::new(BLOCK_SIZE, Arc::new(NoBarrierInterface));
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();
                proxy.open_session(server).unwrap();
                let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Write.into_primitive(),
                            flags: BlockIoFlag::FORCE_ACCESS.bits(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        length: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_eq!(response.status, zx::sys::ZX_ERR_IO);
            }
        );
    }

    /// Verifies that group IDs are isolated per session.
    ///
    /// Even if two independent sessions on the same BlockServer use the same group ID,
    /// their in-flight transaction groups must remain isolated.
    #[fuchsia::test]
    async fn test_group_ids_isolated_per_session() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        futures::join!(
            async {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    // MockInterface::flush() is a no-op that returns Ok(()).
                    Arc::new(MockInterface::default()),
                );
                block_server.handle_requests(stream).await.unwrap();
            },
            async move {
                async fn settle() {
                    // Let the single-threaded executor drain server work.
                    for _ in 0..32 {
                        fasync::yield_now().await;
                    }
                }

                // --- Open session A. ---
                let (session_a, server_a) = fidl::endpoints::create_proxy();
                proxy.open_session(server_a).unwrap();
                let mut fifo_a =
                    fasync::Fifo::from_fifo(session_a.get_fifo().await.unwrap().unwrap());

                // --- Open session B. ---
                let (session_b, server_b) = fidl::endpoints::create_proxy();
                proxy.open_session(server_b).unwrap();
                let mut fifo_b =
                    fasync::Fifo::from_fifo(session_b.get_fifo().await.unwrap().unwrap());

                // ----------------------------------------------------------------
                // Control: with no interference, A's two-part Flush group is OK.
                // ----------------------------------------------------------------
                {
                    let (mut reader_a, mut writer_a) = fifo_a.async_io();
                    writer_a
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Flush.into_primitive(),
                                flags: BlockIoFlag::GROUP_ITEM.bits(),
                                ..Default::default()
                            },
                            group: 1,
                            reqid: 0xAAAA,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                    settle().await;
                    writer_a
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Flush.into_primitive(),
                                flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                                ..Default::default()
                            },
                            group: 1,
                            reqid: 0xAAAA,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                    let mut response = BlockFifoResponse::default();
                    reader_a.read_entries(&mut response).await.unwrap();
                    assert_eq!(response.reqid, 0xAAAA);
                    assert_eq!(
                        response.status,
                        zx::sys::ZX_OK,
                        "control: A's valid Flush group must succeed"
                    );
                }

                // ----------------------------------------------------------------
                // Run concurrent group requests with the same group ID (7) on
                // both sessions, and verify they both succeed independently.
                // ----------------------------------------------------------------

                // Step 1: Session A starts group 7.
                {
                    let (_reader_a, mut writer_a) = fifo_a.async_io();
                    writer_a
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Flush.into_primitive(),
                                flags: BlockIoFlag::GROUP_ITEM.bits(),
                                ..Default::default()
                            },
                            group: 7,
                            reqid: 100,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
                settle().await;

                // Step 2: Session B starts group 7.
                {
                    let (_reader_b, mut writer_b) = fifo_b.async_io();
                    writer_b
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Flush.into_primitive(),
                                flags: BlockIoFlag::GROUP_ITEM.bits(),
                                ..Default::default()
                            },
                            group: 7,
                            reqid: 200,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
                settle().await;

                // Step 3: Session A finishes group 7.
                {
                    let (_reader_a, mut writer_a) = fifo_a.async_io();
                    writer_a
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Flush.into_primitive(),
                                flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                                ..Default::default()
                            },
                            group: 7,
                            reqid: 100,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
                settle().await;

                // Step 4: Session B finishes group 7.
                {
                    let (_reader_b, mut writer_b) = fifo_b.async_io();
                    writer_b
                        .write_entries(&BlockFifoRequest {
                            command: BlockFifoCommand {
                                opcode: BlockOpcode::Flush.into_primitive(),
                                flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                                ..Default::default()
                            },
                            group: 7,
                            reqid: 200,
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
                settle().await;

                // Verify Session A's response.
                {
                    let (mut reader_a, _writer_a) = fifo_a.async_io();
                    let mut response_a = BlockFifoResponse::default();
                    reader_a.read_entries(&mut response_a).await.unwrap();
                    assert_eq!(response_a.reqid, 100);
                    assert_eq!(response_a.group, 7);
                    assert_eq!(response_a.status, zx::sys::ZX_OK);
                }

                // Verify Session B's response.
                {
                    let (mut reader_b, _writer_b) = fifo_b.async_io();
                    let mut response_b = BlockFifoResponse::default();
                    reader_b.read_entries(&mut response_b).await.unwrap();
                    assert_eq!(response_b.reqid, 200);
                    assert_eq!(response_b.group, 7);
                    assert_eq!(response_b.status, zx::sys::ZX_OK);
                }

                std::mem::drop(session_a);
                std::mem::drop(session_b);
                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_unmapped_request_out_of_range() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();
        let _server = fasync::Task::spawn(async move {
            let block_server = BlockServer::new(4096, Arc::new(MockInterface::default()));
            let _ = block_server.handle_requests(stream).await;
        });

        let (session_proxy, server) = fidl::endpoints::create_proxy();
        proxy.open_session(server).unwrap();

        let vmo = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let mut fifo = fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
        let (mut reader, mut writer) = fifo.async_io();

        // Attempting to read at offset 100 on a 100-block unmapped partition should fail with
        // OUT_OF_RANGE.
        writer
            .write_entries(&BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    ..Default::default()
                },
                reqid: 1,
                vmoid: vmo_id.id,
                length: 1,
                dev_offset: 100,
                ..Default::default()
            })
            .await
            .unwrap();

        let mut response = BlockFifoResponse::default();
        reader.read_entries(&mut response).await.unwrap();
        assert_eq!(zx::Status::ok(response.status), Err(zx::Status::OUT_OF_RANGE));
    }

    #[test]
    fn test_offset_map_coalescing() {
        use crate::{BlockOffsetMapping, OffsetMap};

        // Mappings 0 and 1 are contiguous in device offset (100..150 + 150..180 = 100..180).
        // Mapping 2 is not contiguous with 1 (target 250 instead of 180).
        let mappings = vec![
            BlockOffsetMapping { target_block_offset: 100, length: 50 },
            BlockOffsetMapping { target_block_offset: 150, length: 30 },
            BlockOffsetMapping { target_block_offset: 250, length: 20 },
        ];
        let map = OffsetMap::new(crate::coalesce_mappings(mappings)).unwrap();

        assert_eq!(map.mappings().len(), 2);
        assert_eq!(map.mappings()[0].target_block_offset, 100);
        assert_eq!(map.mappings()[0].length, 80);
        assert_eq!(map.mappings()[1].target_block_offset, 250);
        assert_eq!(map.mappings()[1].length, 20);

        // Logical offset 10 falls in the first extent, but since it coalesces with the second,
        // extent_remaining_blocks extends to logical offset 80 (80 - 10 = 70).
        assert_eq!(map.map(10), Some((110, 70)));
        // Logical offset 55 falls in the second extent, remaining blocks up to logical 80
        // (80 - 55 = 25).
        assert_eq!(map.map(55), Some((155, 25)));
        // Logical offset 85 falls in the third extent, which is not coalesced with the second
        // (100 - 85 = 15).
        assert_eq!(map.map(85), Some((255, 15)));
    }
    #[fuchsia::test]
    async fn test_open_session_with_options_errors() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        futures::join!(
            async {
                let block_server = BlockServer::new(BLOCK_SIZE, Arc::new(MockInterface::default()));
                let _ = block_server.handle_requests(stream).await;
            },
            async move {
                // Test 1: Mapping with zero length -> should close session with
                // INVALID_ARGS epitaph.
                {
                    let (session_proxy, server) = fidl::endpoints::create_proxy();
                    proxy
                        .open_session_with_options(
                            server,
                            &[fblock::BlockOffsetMapping { target_block_offset: 0, length: 0 }],
                        )
                        .unwrap();
                    let res = session_proxy.get_fifo().await;
                    assert_matches!(
                        res,
                        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                            if epitaph == zx::Status::INVALID_ARGS
                    );
                }

                // Test 2: Mappings out of range (exceeding block_count) -> should close with
                // OUT_OF_RANGE epitaph.
                {
                    let (session_proxy, server) = fidl::endpoints::create_proxy();
                    let mapping = fblock::BlockOffsetMapping {
                        target_block_offset: u64::MAX - 10, // Way past block_count
                        length: 10,
                    };
                    proxy.open_session_with_options(server, &[mapping]).unwrap();
                    let res = session_proxy.get_fifo().await;
                    assert_matches!(
                        res,
                        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                            if epitaph == zx::Status::OUT_OF_RANGE
                    );
                }
            }
        );
    }

    #[fuchsia::test]
    async fn test_split_request_failure_aborts_subsequent_chunks() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

        let read_calls = Arc::new(AtomicU64::new(0));
        let read_calls_clone = read_calls.clone();

        struct MockSplitInterface {
            read_calls: Arc<AtomicU64>,
        }

        impl super::async_interface::Interface for MockSplitInterface {
            fn get_info(&self) -> Cow<'_, DeviceInfo> {
                Cow::Owned(DeviceInfo::Partition(PartitionInfo {
                    device_flags: fblock::DeviceFlag::READONLY,
                    max_transfer_blocks: NonZero::new(5),
                    start_block_offset: Some(0),
                    block_count: 100,
                    type_guid: [1; 16],
                    instance_guid: [2; 16],
                    name: "foo".to_string(),
                    flags: Some(0),
                }))
            }

            async fn on_attach_vmo(&self, _vmo: &zx::Vmo) -> Result<(), zx::Status> {
                Ok(())
            }

            async fn read(
                &self,
                _device_block_offset: u64,
                _block_count: u32,
                _vmo: &Arc<zx::Vmo>,
                _vmo_offset: u64,
                _opts: ReadOptions,
                _trace_flow_id: TraceFlowId,
            ) -> Result<(), zx::Status> {
                let call_num = self.read_calls.fetch_add(1, Ordering::Relaxed);
                if call_num == 0 { Err(zx::Status::IO) } else { Ok(()) }
            }

            async fn write(
                &self,
                _device_block_offset: u64,
                _block_count: u32,
                _vmo: &Arc<zx::Vmo>,
                _vmo_offset: u64,
                _opts: WriteOptions,
                _trace_flow_id: TraceFlowId,
            ) -> Result<(), zx::Status> {
                unreachable!()
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
                unreachable!()
            }
        }

        futures::join!(
            async move {
                let block_server = BlockServer::new(
                    BLOCK_SIZE,
                    Arc::new(MockSplitInterface { read_calls: read_calls_clone }),
                );
                let _ = block_server.handle_requests(stream).await;
            },
            async move {
                let (session_proxy, server) = fidl::endpoints::create_proxy();
                proxy.open_session(server).unwrap();

                let vmo = zx::Vmo::create(2 * zx::system_get_page_size() as u64).unwrap();
                let vmo_id = session_proxy
                    .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                    .await
                    .unwrap()
                    .unwrap();

                let mut fifo =
                    fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
                let (mut reader, mut writer) = fifo.async_io();

                // Send a request for 10 blocks. With max_transfer_blocks = 5, this will be split
                // into 2 chunks of 5 blocks each.
                writer
                    .write_entries(&BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            ..Default::default()
                        },
                        vmoid: vmo_id.id,
                        length: 10,
                        dev_offset: 0,
                        reqid: 1,
                        ..Default::default()
                    })
                    .await
                    .unwrap();

                let mut response = BlockFifoResponse::default();
                reader.read_entries(&mut response).await.unwrap();
                assert_ne!(response.status, zx::sys::ZX_OK);

                // Verify that only the first chunk was submitted to `read`. The second chunk
                // should have been aborted when `map_request` saw active_request.status != OK.
                assert_eq!(read_calls.load(Ordering::Relaxed), 1);

                std::mem::drop(proxy);
            }
        );
    }

    #[fuchsia::test]
    async fn test_mapper_open_session() {
        use crate::callback_interface::SessionManager;
        use crate::testing::MockInterface;

        let (tx, _rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface, 512));
        let block_server = BlockServer::new(512, session_manager);

        let (mapper_proxy, mapper_stream) =
            fidl::endpoints::create_proxy_and_stream::<fblock::MapperMarker>();
        let scope = fasync::Scope::new();
        scope.spawn(async move {
            let _ = block_server.handle_mapper_requests(mapper_stream).await;
        });

        // 1. Both port and delivery queue provided (with pager):
        let (_mapper_session_proxy, mapper_session_server) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let mapping_vmo = zx::Vmo::create(4096).unwrap();
        let port = zx::Port::create();
        let delivery_queue = zx::Vmo::create(4096).unwrap();
        let res = mapper_proxy
            .open_session(mapper_session_server, mapping_vmo, Some(port), Some(delivery_queue))
            .await
            .unwrap();
        assert_matches!(res, Ok(()));

        // 2. Neither port nor delivery queue provided (pager-less):
        let (_mapper_session_proxy, mapper_session_server) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let mapping_vmo = zx::Vmo::create(4096).unwrap();
        let res = mapper_proxy
            .open_session(mapper_session_server, mapping_vmo, None, None)
            .await
            .unwrap();
        assert_matches!(res, Ok(()));

        // 3. Port provided without delivery queue (invalid):
        let (_mapper_session_proxy, mapper_session_server) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let mapping_vmo = zx::Vmo::create(4096).unwrap();
        let port = zx::Port::create();
        let res = mapper_proxy
            .open_session(mapper_session_server, mapping_vmo, Some(port), None)
            .await
            .unwrap();
        assert_eq!(res, Err(zx::sys::ZX_ERR_INVALID_ARGS));

        // 4. Delivery queue provided without port (invalid):
        let (_mapper_session_proxy, mapper_session_server) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let mapping_vmo = zx::Vmo::create(4096).unwrap();
        let delivery_queue = zx::Vmo::create(4096).unwrap();
        let res = mapper_proxy
            .open_session(mapper_session_server, mapping_vmo, None, Some(delivery_queue))
            .await
            .unwrap();
        assert_eq!(res, Err(zx::sys::ZX_ERR_INVALID_ARGS));
    }

    #[fuchsia::test]
    async fn test_mapper_blob_page_request() {
        use crate::callback_interface::SessionManager;
        use crate::testing::MockInterface;

        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), 512));
        let block_server = BlockServer::new(512, session_manager.clone());

        let sm_completer = session_manager.clone();
        std::thread::spawn(move || {
            while let Ok(req) = rx.recv() {
                if let Operation::Read { vmo_offset, block_count, .. } = req.operation {
                    if let Some(vmo) = req.vmo {
                        let data = vec![0xABu8; (block_count * 512) as usize];
                        vmo.write(&data, vmo_offset).unwrap();
                    }
                }
                sm_completer.complete_request(req.request_id, Ok(()));
            }
        });

        let (mapper_proxy, mapper_stream) =
            fidl::endpoints::create_proxy_and_stream::<fblock::MapperMarker>();
        let scope = fasync::Scope::new();
        scope.spawn(async move {
            let _ = block_server.handle_mapper_requests(mapper_stream).await;
        });

        let pager = Arc::new(zx::Pager::create(zx::PagerOptions::empty()).unwrap());
        let port = zx::Port::create();
        let key = 1001u64;
        let paged_vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, key, 4096).unwrap();

        let (_mapper_session_proxy, mapper_session_server) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let mapping_vmo = zx::Vmo::create(65536).unwrap();
        let delivery_queue = zx::Vmo::create(65536).unwrap();
        let vmo_provider = Arc::new(blob_pager_and_verifier::TestVmoProvider::new(
            pager.clone(),
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        ));
        vmo_provider
            .register_vmo(key, paged_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());
        let receiver = vmo_fifo::Receiver::<mapping::RawDeliveryCommand>::new(
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            mapping::PENDING_DELIVERY_COMMANDS_CAPACITY,
        )
        .unwrap();
        let _delivery_processor = blob_pager_and_verifier::DeliveryQueueProcessor::spawn(
            receiver,
            vmo_provider,
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        )
        .unwrap();

        let extents =
            mapping::Extents::try_new([mapping::Extent::new(0..4096, Some(0))], 0).unwrap();
        let data_extent_words: Vec<u64> = mapping::Extents::encode_extents(&extents).collect();
        let mut payload_bytes = Vec::new();
        for w in &data_extent_words {
            payload_bytes.extend_from_slice(&w.to_le_bytes());
        }

        let cmd = mapping::RawMappingCommand {
            opcode: mapping::MAPPINGS_COMMAND,
            offset: 0,
            key,
            stored_size: 4096,
            device_offset: 0,
            metadata_count: 0,
            extent_count: data_extent_words.len() as u32,
        };

        let mut sender = vmo_fifo::SyncSender::<mapping::RawMappingCommand>::new(
            mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            1024,
            256,
        )
        .unwrap();
        let mut payload_buf = sender.reserve_payload(payload_bytes.len()).unwrap();
        payload_buf.data().copy_from_slice(&payload_bytes);
        payload_buf.commit(cmd).unwrap();

        let res = mapper_proxy
            .open_session(mapper_session_server, mapping_vmo, Some(port), Some(delivery_queue))
            .await
            .unwrap();
        assert_matches!(res, Ok(()));

        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            paged_vmo.read(&mut buf, 0).expect("paged vmo read failed");
            buf
        });

        let read_bytes = reader_thread.join().unwrap();
        assert_eq!(read_bytes.len(), 4096);
        assert_eq!(read_bytes, [0xABu8; 4096]);
    }

    #[fuchsia::test]
    async fn test_mapper_hierarchical_session() {
        use crate::callback_interface::SessionManager;
        use crate::testing::MockInterface;

        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), 512));
        let block_server = BlockServer::new(512, session_manager.clone());

        let sm_completer = session_manager.clone();
        std::thread::spawn(move || {
            while let Ok(req) = rx.recv() {
                if let Operation::Read { vmo_offset, block_count, .. } = req.operation {
                    if let Some(vmo) = req.vmo {
                        let data = vec![0xABu8; (block_count * 512) as usize];
                        vmo.write(&data, vmo_offset).unwrap();
                    }
                }
                sm_completer.complete_request(req.request_id, Ok(()));
            }
        });

        let (mapper_proxy, mapper_stream) =
            fidl::endpoints::create_proxy_and_stream::<fblock::MapperMarker>();
        let scope = fasync::Scope::new();
        scope.spawn(async move {
            let _ = block_server.handle_mapper_requests(mapper_stream).await;
        });

        // 1. Prepare and open intermediate root session without pager.
        let (root_session_proxy, root_session_server) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let root_mapping_vmo = zx::Vmo::create(65536).unwrap();

        // Register partition in root session at key 500:
        // Logical 0..8192 maps to physical device offset 4096..12288.
        let partition_key = 500u64;
        let extents =
            mapping::Extents::try_new([mapping::Extent::new(0..8192, Some(4096))], 0).unwrap();
        let partition_extents: Vec<u64> = mapping::Extents::encode_extents(&extents).collect();
        let mut payload_bytes = Vec::new();
        for w in &partition_extents {
            payload_bytes.extend_from_slice(&w.to_le_bytes());
        }
        let cmd = mapping::RawMappingCommand {
            opcode: mapping::MAPPINGS_COMMAND,
            offset: 0,
            key: partition_key,
            stored_size: 8192,
            device_offset: 0,
            metadata_count: 0,
            extent_count: partition_extents.len() as u32,
        };
        let mut sender = vmo_fifo::SyncSender::<mapping::RawMappingCommand>::new(
            root_mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            1024,
            256,
        )
        .unwrap();
        let mut payload_buf = sender.reserve_payload(payload_bytes.len()).unwrap();
        payload_buf.data().copy_from_slice(&payload_bytes);
        payload_buf.commit(cmd).unwrap();

        let res = mapper_proxy
            .open_session(root_session_server, root_mapping_vmo, None, None)
            .await
            .unwrap();
        assert_matches!(res, Ok(()));

        // 2. Open child session with pager through partition key 500.
        let pager = Arc::new(zx::Pager::create(zx::PagerOptions::empty()).unwrap());
        let port = zx::Port::create();
        let child_key = 2002u64;
        let paged_vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, child_key, 4096).unwrap();

        let child_mapping_vmo = zx::Vmo::create(65536).unwrap();
        let delivery_queue = zx::Vmo::create(65536).unwrap();
        let vmo_provider = Arc::new(blob_pager_and_verifier::TestVmoProvider::new(
            pager.clone(),
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        ));
        vmo_provider
            .register_vmo(child_key, paged_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());
        let receiver = vmo_fifo::Receiver::<mapping::RawDeliveryCommand>::new(
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            mapping::PENDING_DELIVERY_COMMANDS_CAPACITY,
        )
        .unwrap();
        let _delivery_processor = blob_pager_and_verifier::DeliveryQueueProcessor::spawn(
            receiver,
            vmo_provider,
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        )
        .unwrap();

        let extents =
            mapping::Extents::try_new([mapping::Extent::new(0..4096, Some(0))], 0).unwrap();
        let child_extents: Vec<u64> = mapping::Extents::encode_extents(&extents).collect();
        let mut child_payload_bytes = Vec::new();
        for w in &child_extents {
            child_payload_bytes.extend_from_slice(&w.to_le_bytes());
        }
        let child_cmd = mapping::RawMappingCommand {
            opcode: mapping::MAPPINGS_COMMAND,
            offset: 0,
            key: child_key,
            stored_size: 4096,
            device_offset: 0,
            metadata_count: 0,
            extent_count: child_extents.len() as u32,
        };
        let mut child_sender = vmo_fifo::SyncSender::<mapping::RawMappingCommand>::new(
            child_mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            1024,
            256,
        )
        .unwrap();
        let mut child_payload_buf =
            child_sender.reserve_payload(child_payload_bytes.len()).unwrap();
        child_payload_buf.data().copy_from_slice(&child_payload_bytes);
        child_payload_buf.commit(child_cmd).unwrap();

        let mut child_res = Err(zx::Status::NOT_FOUND);
        for _ in 0..100 {
            let (_child_session_proxy, child_session_server) =
                fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
            let child_res_raw = root_session_proxy
                .open_child_session(
                    child_session_server,
                    child_mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                    partition_key,
                    Some(port.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                    Some(delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                )
                .await
                .unwrap();
            if child_res_raw.is_ok() {
                child_res = Ok(());
                break;
            }
            fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        }
        assert_matches!(child_res, Ok(()));

        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            paged_vmo.read(&mut buf, 0).expect("paged vmo read failed");
            buf
        });

        let read_bytes = reader_thread.join().unwrap();
        assert_eq!(read_bytes.len(), 4096);
        assert_eq!(read_bytes, [0xABu8; 4096]);
    }

    #[fuchsia::test]
    async fn test_mapper_child_session_closed_when_parent_closed() {
        use crate::callback_interface::SessionManager;
        use crate::testing::MockInterface;
        use fidl::endpoints::Proxy as _;

        let (tx, _rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), 512));
        let block_server = BlockServer::new(512, session_manager.clone());

        let (mapper_proxy, mapper_stream) =
            fidl::endpoints::create_proxy_and_stream::<fblock::MapperMarker>();
        let scope = fasync::Scope::new();
        scope.spawn(async move {
            let _ = block_server.handle_mapper_requests(mapper_stream).await;
        });

        // 1. Prepare and open intermediate root session without pager.
        let (root_session_proxy, root_session_server) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let root_mapping_vmo = zx::Vmo::create(65536).unwrap();

        let partition_key = 500u64;
        let extents =
            mapping::Extents::try_new([mapping::Extent::new(0..8192, Some(4096))], 0).unwrap();
        let partition_extents: Vec<u64> = mapping::Extents::encode_extents(&extents).collect();
        let mut payload_bytes = Vec::new();
        for w in &partition_extents {
            payload_bytes.extend_from_slice(&w.to_le_bytes());
        }
        let cmd = mapping::RawMappingCommand {
            opcode: mapping::MAPPINGS_COMMAND,
            offset: 0,
            key: partition_key,
            stored_size: 8192,
            device_offset: 0,
            metadata_count: 0,
            extent_count: partition_extents.len() as u32,
        };
        let mut sender = vmo_fifo::SyncSender::<mapping::RawMappingCommand>::new(
            root_mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            1024,
            256,
        )
        .unwrap();
        let mut payload_buf = sender.reserve_payload(payload_bytes.len()).unwrap();
        payload_buf.data().copy_from_slice(&payload_bytes);
        payload_buf.commit(cmd).unwrap();

        let res = mapper_proxy
            .open_session(root_session_server, root_mapping_vmo, None, None)
            .await
            .unwrap();
        assert_matches!(res, Ok(()));

        // 2. Open child session.
        let child_mapping_vmo = zx::Vmo::create(65536).unwrap();
        let mut child_session = None;
        for _ in 0..100 {
            let (child_session_proxy, child_session_server) =
                fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
            let child_res_raw = root_session_proxy
                .open_child_session(
                    child_session_server,
                    child_mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                    partition_key,
                    None,
                    None,
                )
                .await
                .unwrap();
            if child_res_raw.is_ok() {
                child_session = Some(child_session_proxy);
                break;
            }
            fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        }
        let child_session_proxy = child_session.expect("Failed to open child session");

        // Close the parent session.
        root_session_proxy.close().await.unwrap().expect("Close parent session failed");

        // The child session should be closed as well.
        child_session_proxy.on_closed().await.unwrap();
    }
}
