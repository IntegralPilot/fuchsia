// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A Rust client library for the Fuchsia block protocol.
//!
//! This crate provides a low-level client for interacting with block devices over the
//! `fuchsia.hardware.block` FIDL protocol and the FIFO interface for block I/O.
//!
//! See the [`BlockClient`] trait.

use fidl_fuchsia_storage_block as block;
use fidl_fuchsia_storage_block::{MAX_TRANSFER_UNBOUNDED, VMOID_INVALID};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::executor::block_on;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ops::{DerefMut, Range};
use std::pin::Pin;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll, Waker};
use storage_trace as trace;
use zx::sys::zx_handle_t;

pub use cache::Cache;

pub use block::DeviceFlag as BlockDeviceFlag;

pub use block_protocol::*;

pub mod cache;

const TEMP_VMO_SIZE: usize = 65536;

/// If a trace flow ID isn't specified for requests, one will be generated.
pub const NO_TRACE_ID: u64 = 0;

pub use fidl_fuchsia_storage_block::{BlockIoFlag, BlockOpcode};

fn fidl_to_status(error: fidl::Error) -> zx::Status {
    match error {
        fidl::Error::ClientChannelClosed { epitaph, .. } => match epitaph.into() {
            Err(s) => s,
            Ok(()) => zx::Status::PEER_CLOSED,
        },
        _ => zx::Status::INTERNAL,
    }
}

fn opcode_str(opcode: u8) -> &'static str {
    match BlockOpcode::from_primitive(opcode) {
        Some(BlockOpcode::Read) => "read",
        Some(BlockOpcode::Write) => "write",
        Some(BlockOpcode::Flush) => "flush",
        Some(BlockOpcode::Trim) => "trim",
        Some(BlockOpcode::CloseVmo) => "close_vmo",
        None => "unknown",
    }
}

// Generates a trace ID that will be unique across the system (as long as |request_id| isn't
// reused within this process).
fn generate_trace_flow_id(request_id: u32) -> u64 {
    static SELF_HANDLE: LazyLock<zx_handle_t> =
        LazyLock::new(|| fuchsia_runtime::process_self().raw_handle());
    *SELF_HANDLE as u64 + (request_id as u64) << 32
}

pub enum BufferSlice<'a> {
    VmoId { vmo_id: &'a VmoId, offset: u64, length: u64 },
    Memory(&'a [u8]),
}

impl<'a> BufferSlice<'a> {
    pub fn new_with_vmo_id(vmo_id: &'a VmoId, offset: u64, length: u64) -> Self {
        BufferSlice::VmoId { vmo_id, offset, length }
    }
}

impl<'a> From<&'a [u8]> for BufferSlice<'a> {
    fn from(buf: &'a [u8]) -> Self {
        BufferSlice::Memory(buf)
    }
}

pub enum MutableBufferSlice<'a> {
    VmoId { vmo_id: &'a VmoId, offset: u64, length: u64 },
    Memory(&'a mut [u8]),
}

impl<'a> MutableBufferSlice<'a> {
    pub fn new_with_vmo_id(vmo_id: &'a VmoId, offset: u64, length: u64) -> Self {
        MutableBufferSlice::VmoId { vmo_id, offset, length }
    }
}

impl<'a> From<&'a mut [u8]> for MutableBufferSlice<'a> {
    fn from(buf: &'a mut [u8]) -> Self {
        MutableBufferSlice::Memory(buf)
    }
}

#[derive(Default)]
struct RequestState {
    result: Option<Result<(), zx::Status>>,
    waker: Option<Waker>,
}

#[derive(Default)]
struct FifoState {
    // The fifo.
    fifo: Option<fasync::Fifo<BlockFifoResponse, BlockFifoRequest>>,

    // The next request ID to be used.
    next_request_id: u32,

    // A queue of messages to be sent on the fifo.
    queue: std::collections::VecDeque<BlockFifoRequest>,

    // Map from request ID to RequestState.
    map: HashMap<u32, RequestState>,

    // The waker for the FifoPoller.
    poller_waker: Option<Waker>,
}

impl FifoState {
    fn terminate(&mut self) {
        self.fifo.take();
        for (_, request_state) in self.map.iter_mut() {
            request_state.result.get_or_insert(Err(zx::Status::CANCELED));
            if let Some(waker) = request_state.waker.take() {
                waker.wake();
            }
        }
        if let Some(waker) = self.poller_waker.take() {
            waker.wake();
        }
    }

    // Returns true if polling should be terminated.
    fn poll_send_requests(&mut self, context: &mut Context<'_>) -> bool {
        let fifo = if let Some(fifo) = self.fifo.as_ref() {
            fifo
        } else {
            return true;
        };

        loop {
            let slice = self.queue.as_slices().0;
            if slice.is_empty() {
                return false;
            }
            match fifo.try_write(context, slice) {
                Poll::Ready(Ok(sent)) => {
                    self.queue.drain(0..sent.get());
                }
                Poll::Ready(Err(_)) => {
                    self.terminate();
                    return true;
                }
                Poll::Pending => {
                    return false;
                }
            }
        }
    }
}

type FifoStateRef = Arc<Mutex<FifoState>>;

// A future used for fifo responses.
struct ResponseFuture {
    request_id: u32,
    fifo_state: FifoStateRef,
}

impl ResponseFuture {
    fn new(fifo_state: FifoStateRef, request_id: u32) -> Self {
        ResponseFuture { request_id, fifo_state }
    }
}

impl Future for ResponseFuture {
    type Output = Result<(), zx::Status>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.fifo_state.lock();
        let request_state = state.map.get_mut(&self.request_id).unwrap();
        if let Some(result) = request_state.result {
            Poll::Ready(result)
        } else {
            request_state.waker.replace(context.waker().clone());
            Poll::Pending
        }
    }
}

impl Drop for ResponseFuture {
    fn drop(&mut self) {
        let mut state = self.fifo_state.lock();
        if let Some(request_state) = state.map.remove(&self.request_id) {
            if request_state.result.is_none() {
                // Request was still pending!  This is a cancellation which we do not support.  We
                // do not know the disposition of any VMO that the far end might still be writing
                // to.  To avoid potential corruption (e.g. a client reuses a buffer that might
                // still be being used by the driver), terminate the connection to prevent any
                // future use.
                state.terminate();
            }
        }
        update_outstanding_requests_counter(state.map.len());
    }
}

/// Wraps a vmo-id. Will panic if you forget to detach.
#[derive(Debug)]
#[must_use]
pub struct VmoId(AtomicU16);

impl VmoId {
    /// VmoIds will normally be vended by attach_vmo, but this might be used in some tests
    pub fn new(id: u16) -> Self {
        Self(AtomicU16::new(id))
    }

    /// Invalidates self and returns a new VmoId with the same underlying ID.
    pub fn take(&self) -> Self {
        Self(AtomicU16::new(self.0.swap(VMOID_INVALID, Ordering::Relaxed)))
    }

    pub fn is_valid(&self) -> bool {
        self.id() != VMOID_INVALID
    }

    /// Takes the ID.  The caller assumes responsibility for detaching.
    #[must_use]
    pub fn into_id(self) -> u16 {
        self.0.swap(VMOID_INVALID, Ordering::Relaxed)
    }

    pub fn id(&self) -> u16 {
        self.0.load(Ordering::Relaxed)
    }
}

impl PartialEq for VmoId {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl Eq for VmoId {}

impl Drop for VmoId {
    fn drop(&mut self) {
        assert_eq!(self.0.load(Ordering::Relaxed), VMOID_INVALID, "Did you forget to detach?");
    }
}

impl Hash for VmoId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

/// Represents a client connection to a block device. This is a simplified version of the block.fidl
/// interface.
/// Most users will use the RemoteBlockClient instantiation of this trait.
pub trait BlockClient: Send + Sync {
    /// Wraps AttachVmo from fuchsia.hardware.block::Block.
    ///
    /// # Safety
    ///
    /// The caller must ensure that no references are held during I/O as this would be
    /// undefined behavior.  The caller may hold pointers, which does not lead to undefined
    /// behavior; Rust does not make the same assumptions as references for pointers.
    ///
    /// Whilst a connection failure or client-side cancellation does not immediately lead to
    /// Rust undefined behavior, the caller must thereafter assume the VMO is poisoned.  No
    /// assumptions can be made regarding what might be written to the VMO by the far end, or
    /// anything the far end might have delegated access to.  The only safe thing to do is
    /// discard the VMO and not attempt to use or re-attach it.
    ///
    /// Attaching the VMO once for its lifetime is safe, if strictly more than necessary.
    unsafe fn attach_vmo(
        &self,
        vmo: &zx::Vmo,
    ) -> impl Future<Output = Result<VmoId, zx::Status>> + Send;

    /// Detaches the given vmo-id from the device.
    fn detach_vmo(&self, vmo_id: VmoId) -> impl Future<Output = Result<(), zx::Status>> + Send;

    /// Reads from the device at |device_offset| into the given buffer slice.
    fn read_at(
        &self,
        buffer_slice: MutableBufferSlice<'_>,
        device_offset: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send {
        self.read_at_with_opts_traced(buffer_slice, device_offset, ReadOptions::default(), 0)
    }

    fn read_at_with_opts(
        &self,
        buffer_slice: MutableBufferSlice<'_>,
        device_offset: u64,
        opts: ReadOptions,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send {
        self.read_at_with_opts_traced(buffer_slice, device_offset, opts, 0)
    }

    fn read_at_with_opts_traced(
        &self,
        buffer_slice: MutableBufferSlice<'_>,
        device_offset: u64,
        opts: ReadOptions,
        trace_flow_id: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send;

    /// Writes the data in |buffer_slice| to the device.
    fn write_at(
        &self,
        buffer_slice: BufferSlice<'_>,
        device_offset: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send {
        self.write_at_with_opts_traced(
            buffer_slice,
            device_offset,
            WriteOptions::default(),
            NO_TRACE_ID,
        )
    }

    fn write_at_with_opts(
        &self,
        buffer_slice: BufferSlice<'_>,
        device_offset: u64,
        opts: WriteOptions,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send {
        self.write_at_with_opts_traced(buffer_slice, device_offset, opts, NO_TRACE_ID)
    }

    fn write_at_with_opts_traced(
        &self,
        buffer_slice: BufferSlice<'_>,
        device_offset: u64,
        opts: WriteOptions,
        trace_flow_id: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send;

    /// Trims the given range on the block device.
    fn trim(
        &self,
        device_range: Range<u64>,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send {
        self.trim_traced(device_range, NO_TRACE_ID)
    }

    fn trim_traced(
        &self,
        device_range: Range<u64>,
        trace_flow_id: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send;

    fn flush(&self) -> impl Future<Output = Result<(), zx::Status>> + Send {
        self.flush_traced(NO_TRACE_ID)
    }

    /// Sends a flush request to the underlying block device.
    fn flush_traced(
        &self,
        trace_flow_id: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> + Send;

    /// Closes the fifo.
    fn close(&self) -> impl Future<Output = Result<(), zx::Status>> + Send;

    /// Returns the block size of the device.
    fn block_size(&self) -> u32;

    /// Returns the size, in blocks, of the device.
    fn block_count(&self) -> u64;

    /// Returns the maximum number of blocks which can be transferred in a single request.
    fn max_transfer_blocks(&self) -> Option<NonZero<u32>>;

    /// Returns the block flags reported by the device.
    fn block_flags(&self) -> BlockDeviceFlag;

    /// Returns true if the remote fifo is still connected.
    fn is_connected(&self) -> bool;
}

struct Common {
    block_size: u32,
    block_count: u64,
    max_transfer_blocks: Option<NonZero<u32>>,
    block_flags: BlockDeviceFlag,
    fifo_state: FifoStateRef,
    temp_vmo: futures::lock::Mutex<zx::Vmo>,
    temp_vmo_id: VmoId,
}

impl Common {
    fn new(
        fifo: fasync::Fifo<BlockFifoResponse, BlockFifoRequest>,
        info: &block::BlockInfo,
        temp_vmo: zx::Vmo,
        temp_vmo_id: VmoId,
    ) -> Self {
        let fifo_state = Arc::new(Mutex::new(FifoState { fifo: Some(fifo), ..Default::default() }));
        fasync::Task::spawn(FifoPoller { fifo_state: fifo_state.clone() }).detach();
        Self {
            block_size: info.block_size,
            block_count: info.block_count,
            max_transfer_blocks: if info.max_transfer_size != MAX_TRANSFER_UNBOUNDED {
                NonZero::new(info.max_transfer_size / info.block_size)
            } else {
                None
            },
            block_flags: info.flags,
            fifo_state,
            temp_vmo: futures::lock::Mutex::new(temp_vmo),
            temp_vmo_id,
        }
    }

    fn to_blocks(&self, bytes: u64) -> Result<u64, zx::Status> {
        if bytes % self.block_size as u64 != 0 {
            Err(zx::Status::INVALID_ARGS)
        } else {
            Ok(bytes / self.block_size as u64)
        }
    }

    // Sends the request and waits for the response.
    async fn send(&self, mut request: BlockFifoRequest) -> Result<(), zx::Status> {
        let (request_id, trace_flow_id) = {
            let mut state = self.fifo_state.lock();

            if state.fifo.is_none() {
                // Fifo has been closed.
                return Err(zx::Status::CANCELED);
            }
            trace::duration!(
                "storage",
                "block_client::send::start",
                "op" => opcode_str(request.command.opcode),
                "len" => request.length * self.block_size
            );
            let request_id = state.next_request_id;
            state.next_request_id = state.next_request_id.overflowing_add(1).0;
            assert!(
                state.map.insert(request_id, RequestState::default()).is_none(),
                "request id in use!"
            );
            update_outstanding_requests_counter(state.map.len());
            request.reqid = request_id;
            if request.trace_flow_id == NO_TRACE_ID {
                request.trace_flow_id = generate_trace_flow_id(request_id);
            }
            let trace_flow_id = request.trace_flow_id;
            trace::flow_begin!("storage", "block_client::send", trace_flow_id.into());
            state.queue.push_back(request);
            if let Some(waker) = state.poller_waker.clone() {
                state.poll_send_requests(&mut Context::from_waker(&waker));
            }
            (request_id, trace_flow_id)
        };
        ResponseFuture::new(self.fifo_state.clone(), request_id).await?;
        trace::duration!("storage", "block_client::send::end");
        trace::flow_end!("storage", "block_client::send", trace_flow_id.into());
        Ok(())
    }

    fn detach_vmo(&self, vmo_id: VmoId) -> impl Future<Output = Result<(), zx::Status>> {
        self.send(BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::CloseVmo.into_primitive(),
                flags: 0,
                ..Default::default()
            },
            vmoid: vmo_id.into_id(),
            ..Default::default()
        })
    }

    async fn read_at(
        &self,
        buffer_slice: MutableBufferSlice<'_>,
        device_offset: u64,
        opts: ReadOptions,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        let mut flags = BlockIoFlag::empty();

        if opts.inline_crypto.is_enabled {
            flags |= BlockIoFlag::INLINE_ENCRYPTION_ENABLED;
        }

        match buffer_slice {
            MutableBufferSlice::VmoId { vmo_id, offset, length } => {
                self.send(BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Read.into_primitive(),
                        flags: flags.bits(),
                        ..Default::default()
                    },
                    vmoid: vmo_id.id(),
                    length: self
                        .to_blocks(length)?
                        .try_into()
                        .map_err(|_| zx::Status::INVALID_ARGS)?,
                    vmo_offset: self.to_blocks(offset)?,
                    dev_offset: self.to_blocks(device_offset)?,
                    trace_flow_id,
                    dun: opts.inline_crypto.dun,
                    slot: opts.inline_crypto.slot,
                    ..Default::default()
                })
                .await?
            }
            MutableBufferSlice::Memory(mut slice) => {
                let temp_vmo = self.temp_vmo.lock().await;
                let mut device_block = self.to_blocks(device_offset)?;
                loop {
                    let to_do = std::cmp::min(TEMP_VMO_SIZE, slice.len());
                    let block_count = self.to_blocks(to_do as u64)? as u32;
                    self.send(BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Read.into_primitive(),
                            flags: flags.bits(),
                            ..Default::default()
                        },
                        vmoid: self.temp_vmo_id.id(),
                        length: block_count,
                        vmo_offset: 0,
                        dev_offset: device_block,
                        trace_flow_id,
                        dun: opts.inline_crypto.dun,
                        slot: opts.inline_crypto.slot,
                        ..Default::default()
                    })
                    .await?;
                    temp_vmo.read(&mut slice[..to_do], 0)?;
                    if to_do == slice.len() {
                        break;
                    }
                    device_block += block_count as u64;
                    slice = &mut slice[to_do..];
                }
            }
        }
        Ok(())
    }

    async fn write_at(
        &self,
        buffer_slice: BufferSlice<'_>,
        device_offset: u64,
        opts: WriteOptions,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        let mut flags = BlockIoFlag::empty();

        if opts.flags.contains(WriteFlags::FORCE_ACCESS) {
            flags |= BlockIoFlag::FORCE_ACCESS;
        }

        if opts.flags.contains(WriteFlags::PRE_BARRIER) {
            flags |= BlockIoFlag::PRE_BARRIER;
        }

        if opts.inline_crypto.is_enabled {
            flags |= BlockIoFlag::INLINE_ENCRYPTION_ENABLED;
        }

        match buffer_slice {
            BufferSlice::VmoId { vmo_id, offset, length } => {
                self.send(BlockFifoRequest {
                    command: BlockFifoCommand {
                        opcode: BlockOpcode::Write.into_primitive(),
                        flags: flags.bits(),
                        ..Default::default()
                    },
                    vmoid: vmo_id.id(),
                    length: self
                        .to_blocks(length)?
                        .try_into()
                        .map_err(|_| zx::Status::INVALID_ARGS)?,
                    vmo_offset: self.to_blocks(offset)?,
                    dev_offset: self.to_blocks(device_offset)?,
                    trace_flow_id,
                    dun: opts.inline_crypto.dun,
                    slot: opts.inline_crypto.slot,
                    ..Default::default()
                })
                .await?;
            }
            BufferSlice::Memory(mut slice) => {
                let temp_vmo = self.temp_vmo.lock().await;
                let mut device_block = self.to_blocks(device_offset)?;
                loop {
                    let to_do = std::cmp::min(TEMP_VMO_SIZE, slice.len());
                    let block_count = self.to_blocks(to_do as u64)? as u32;
                    temp_vmo.write(&slice[..to_do], 0)?;
                    self.send(BlockFifoRequest {
                        command: BlockFifoCommand {
                            opcode: BlockOpcode::Write.into_primitive(),
                            flags: flags.bits(),
                            ..Default::default()
                        },
                        vmoid: self.temp_vmo_id.id(),
                        length: block_count,
                        vmo_offset: 0,
                        dev_offset: device_block,
                        trace_flow_id,
                        dun: opts.inline_crypto.dun,
                        slot: opts.inline_crypto.slot,
                        ..Default::default()
                    })
                    .await?;
                    if to_do == slice.len() {
                        break;
                    }
                    device_block += block_count as u64;
                    slice = &slice[to_do..];
                }
            }
        }
        Ok(())
    }

    async fn trim(&self, device_range: Range<u64>, trace_flow_id: u64) -> Result<(), zx::Status> {
        let length = self.to_blocks(device_range.end - device_range.start)? as u32;
        let dev_offset = self.to_blocks(device_range.start)?;
        self.send(BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Trim.into_primitive(),
                flags: 0,
                ..Default::default()
            },
            vmoid: VMOID_INVALID,
            length,
            dev_offset,
            trace_flow_id,
            ..Default::default()
        })
        .await
    }

    fn flush(&self, trace_flow_id: u64) -> impl Future<Output = Result<(), zx::Status>> {
        self.send(BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Flush.into_primitive(),
                flags: 0,
                ..Default::default()
            },
            vmoid: VMOID_INVALID,
            trace_flow_id,
            ..Default::default()
        })
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }

    fn max_transfer_blocks(&self) -> Option<NonZero<u32>> {
        self.max_transfer_blocks.clone()
    }

    fn block_flags(&self) -> BlockDeviceFlag {
        self.block_flags
    }

    fn is_connected(&self) -> bool {
        self.fifo_state.lock().fifo.is_some()
    }
}

impl Drop for Common {
    fn drop(&mut self) {
        // It's OK to leak the VMO id because the server will dump all VMOs when the fifo is torn
        // down.
        let _ = self.temp_vmo_id.take().into_id();
        self.fifo_state.lock().terminate();
    }
}

// RemoteBlockClient is a BlockClient that communicates with a real block device over FIDL.
pub struct RemoteBlockClient {
    session: block::SessionProxy,
    common: Common,
}

impl RemoteBlockClient {
    /// Returns a connection to a remote block device via the given channel.
    pub async fn new(remote: impl Borrow<block::BlockProxy>) -> Result<Self, zx::Status> {
        let remote = remote.borrow();
        let info =
            remote.get_info().await.map_err(fidl_to_status)?.map_err(zx::Status::err_from_raw)?;
        let (session, server) = fidl::endpoints::create_proxy();
        let () = remote.open_session(server).map_err(fidl_to_status)?;
        Self::from_session(info, session).await
    }

    pub async fn from_session(
        info: block::BlockInfo,
        session: block::SessionProxy,
    ) -> Result<Self, zx::Status> {
        const SCRATCH_VMO_NAME: zx::Name = zx::Name::new_lossy("block-client-scratch-vmo");
        let fifo =
            session.get_fifo().await.map_err(fidl_to_status)?.map_err(zx::Status::err_from_raw)?;
        let fifo = fasync::Fifo::from_fifo(fifo);
        let temp_vmo = zx::Vmo::create(TEMP_VMO_SIZE as u64)?;
        temp_vmo.set_name(&SCRATCH_VMO_NAME)?;
        let dup = temp_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        let vmo_id = session
            .attach_vmo(dup)
            .await
            .map_err(fidl_to_status)?
            .map_err(zx::Status::err_from_raw)?;
        let vmo_id = VmoId::new(vmo_id.id);
        Ok(RemoteBlockClient { session, common: Common::new(fifo, &info, temp_vmo, vmo_id) })
    }
}

impl BlockClient for RemoteBlockClient {
    async unsafe fn attach_vmo(&self, vmo: &zx::Vmo) -> Result<VmoId, zx::Status> {
        let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        let vmo_id = self
            .session
            .attach_vmo(dup)
            .await
            .map_err(fidl_to_status)?
            .map_err(zx::Status::err_from_raw)?;
        Ok(VmoId::new(vmo_id.id))
    }

    fn detach_vmo(&self, vmo_id: VmoId) -> impl Future<Output = Result<(), zx::Status>> {
        self.common.detach_vmo(vmo_id)
    }

    fn read_at_with_opts_traced(
        &self,
        buffer_slice: MutableBufferSlice<'_>,
        device_offset: u64,
        opts: ReadOptions,
        trace_flow_id: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> {
        self.common.read_at(buffer_slice, device_offset, opts, trace_flow_id)
    }

    fn write_at_with_opts_traced(
        &self,
        buffer_slice: BufferSlice<'_>,
        device_offset: u64,
        opts: WriteOptions,
        trace_flow_id: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> {
        self.common.write_at(buffer_slice, device_offset, opts, trace_flow_id)
    }

    fn trim_traced(
        &self,
        range: Range<u64>,
        trace_flow_id: u64,
    ) -> impl Future<Output = Result<(), zx::Status>> {
        self.common.trim(range, trace_flow_id)
    }

    fn flush_traced(&self, trace_flow_id: u64) -> impl Future<Output = Result<(), zx::Status>> {
        self.common.flush(trace_flow_id)
    }

    async fn close(&self) -> Result<(), zx::Status> {
        let () = self
            .session
            .close()
            .await
            .map_err(fidl_to_status)?
            .map_err(zx::Status::err_from_raw)?;
        Ok(())
    }

    fn block_size(&self) -> u32 {
        self.common.block_size()
    }

    fn block_count(&self) -> u64 {
        self.common.block_count()
    }

    fn max_transfer_blocks(&self) -> Option<NonZero<u32>> {
        self.common.max_transfer_blocks()
    }

    fn block_flags(&self) -> BlockDeviceFlag {
        self.common.block_flags()
    }

    fn is_connected(&self) -> bool {
        self.common.is_connected()
    }
}

pub struct RemoteBlockClientSync {
    session: block::SessionSynchronousProxy,
    common: Common,
}

impl RemoteBlockClientSync {
    /// Returns a connection to a remote block device via the given channel, but spawns a separate
    /// thread for polling the fifo which makes it work in cases where no executor is configured for
    /// the calling thread.
    pub fn new(
        client_end: fidl::endpoints::ClientEnd<block::BlockMarker>,
    ) -> Result<Self, zx::Status> {
        let remote = block::BlockSynchronousProxy::new(client_end.into_channel());
        let info = remote
            .get_info(zx::MonotonicInstant::INFINITE)
            .map_err(fidl_to_status)?
            .map_err(zx::Status::err_from_raw)?;
        let (client, server) = fidl::endpoints::create_endpoints();
        let () = remote.open_session(server).map_err(fidl_to_status)?;
        let session = block::SessionSynchronousProxy::new(client.into_channel());
        let fifo = session
            .get_fifo(zx::MonotonicInstant::INFINITE)
            .map_err(fidl_to_status)?
            .map_err(zx::Status::err_from_raw)?;
        let temp_vmo = zx::Vmo::create(TEMP_VMO_SIZE as u64)?;
        let dup = temp_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        let vmo_id = session
            .attach_vmo(dup, zx::MonotonicInstant::INFINITE)
            .map_err(fidl_to_status)?
            .map_err(zx::Status::err_from_raw)?;
        let vmo_id = VmoId::new(vmo_id.id);

        // The fifo needs to be instantiated from the thread that has the executor as that's where
        // the fifo registers for notifications to be delivered.
        let (sender, receiver) = oneshot::channel::<Result<Self, zx::Status>>();
        std::thread::spawn(move || {
            let mut executor = fasync::LocalExecutor::default();
            let fifo = fasync::Fifo::from_fifo(fifo);
            let common = Common::new(fifo, &info, temp_vmo, vmo_id);
            let fifo_state = common.fifo_state.clone();
            let _ = sender.send(Ok(RemoteBlockClientSync { session, common }));
            executor.run_singlethreaded(FifoPoller { fifo_state });
        });
        block_on(receiver).map_err(|_| zx::Status::CANCELED)?
    }

    /// Wraps AttachVmo from fuchsia.hardware.block::Block.
    ///
    /// # Safety
    ///
    /// See `BlockClient::attach_vmo`.
    pub unsafe fn attach_vmo(&self, vmo: &zx::Vmo) -> Result<VmoId, zx::Status> {
        let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        let vmo_id = self
            .session
            .attach_vmo(dup, zx::MonotonicInstant::INFINITE)
            .map_err(fidl_to_status)?
            .map_err(zx::Status::err_from_raw)?;
        Ok(VmoId::new(vmo_id.id))
    }

    pub fn detach_vmo(&self, vmo_id: VmoId) -> Result<(), zx::Status> {
        block_on(self.common.detach_vmo(vmo_id))
    }

    pub fn read_at(
        &self,
        buffer_slice: MutableBufferSlice<'_>,
        device_offset: u64,
    ) -> Result<(), zx::Status> {
        block_on(self.common.read_at(
            buffer_slice,
            device_offset,
            ReadOptions::default(),
            NO_TRACE_ID,
        ))
    }

    pub fn write_at(
        &self,
        buffer_slice: BufferSlice<'_>,
        device_offset: u64,
    ) -> Result<(), zx::Status> {
        block_on(self.common.write_at(
            buffer_slice,
            device_offset,
            WriteOptions::default(),
            NO_TRACE_ID,
        ))
    }

    pub fn flush(&self) -> Result<(), zx::Status> {
        block_on(self.common.flush(NO_TRACE_ID))
    }

    pub fn close(&self) -> Result<(), zx::Status> {
        let () = self
            .session
            .close(zx::MonotonicInstant::INFINITE)
            .map_err(fidl_to_status)?
            .map_err(zx::Status::err_from_raw)?;
        Ok(())
    }

    pub fn block_size(&self) -> u32 {
        self.common.block_size()
    }

    pub fn block_count(&self) -> u64 {
        self.common.block_count()
    }

    pub fn is_connected(&self) -> bool {
        self.common.is_connected()
    }
}

impl Drop for RemoteBlockClientSync {
    fn drop(&mut self) {
        // Ignore errors here as there is not much we can do about it.
        let _ = self.close();
    }
}

// FifoPoller is a future responsible for sending and receiving from the fifo.
struct FifoPoller {
    fifo_state: FifoStateRef,
}

impl Future for FifoPoller {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state_lock = self.fifo_state.lock();
        let state = state_lock.deref_mut(); // So that we can split the borrow.

        // Send requests.
        if state.poll_send_requests(context) {
            return Poll::Ready(());
        }

        // Receive responses.
        let fifo = state.fifo.as_ref().unwrap(); // Safe because poll_send_requests checks.
        loop {
            let mut response = MaybeUninit::uninit();
            match fifo.try_read(context, &mut response) {
                Poll::Pending => {
                    state.poller_waker = Some(context.waker().clone());
                    return Poll::Pending;
                }
                Poll::Ready(Ok(_)) => {
                    let response = unsafe { response.assume_init() };
                    let request_id = response.reqid;
                    // If the request isn't in the map, assume that it's a cancelled read.
                    if let Some(request_state) = state.map.get_mut(&request_id) {
                        request_state.result.replace(zx::Status::ok(response.status));
                        if let Some(waker) = request_state.waker.take() {
                            waker.wake();
                        }
                    }
                }
                Poll::Ready(Err(_)) => {
                    state.terminate();
                    return Poll::Ready(());
                }
            }
        }
    }
}

fn update_outstanding_requests_counter(outstanding: usize) {
    trace::counter!("storage", "block-requests", 0, "outstanding" => outstanding);
}

#[cfg(test)]
mod tests {
    use super::{
        BlockClient, BlockFifoRequest, BlockFifoResponse, BufferSlice, MutableBufferSlice,
        RemoteBlockClient, RemoteBlockClientSync, WriteOptions,
    };
    use block_protocol::ReadOptions;
    use block_server::{BlockServer, DeviceInfo, PartitionInfo};
    use fidl::endpoints::RequestStream as _;
    use fidl_fuchsia_storage_block as block;
    use fuchsia_async as fasync;
    use futures::future::{AbortHandle, Abortable, TryFutureExt as _};
    use futures::join;
    use futures::stream::StreamExt as _;
    use futures::stream::futures_unordered::FuturesUnordered;
    use ramdevice_client::RamdiskClient;
    use std::borrow::Cow;
    use std::num::NonZero;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    const RAMDISK_BLOCK_SIZE: u64 = 1024;
    const RAMDISK_BLOCK_COUNT: u64 = 1024;

    pub async fn make_ramdisk() -> (RamdiskClient, block::BlockProxy, RemoteBlockClient) {
        let ramdisk = RamdiskClient::create(RAMDISK_BLOCK_SIZE, RAMDISK_BLOCK_COUNT)
            .await
            .expect("RamdiskClient::create failed");
        let client_end = ramdisk.open().expect("ramdisk.open failed");
        let proxy = client_end.into_proxy();
        let block_client = RemoteBlockClient::new(proxy).await.expect("new failed");
        assert_eq!(block_client.block_size(), 1024);
        let client_end = ramdisk.open().expect("ramdisk.open failed");
        let proxy = client_end.into_proxy();
        (ramdisk, proxy, block_client)
    }

    #[fuchsia::test]
    async fn test_against_ram_disk() {
        let (_ramdisk, _block_proxy, block_client) = make_ramdisk().await;

        let vmo = zx::Vmo::create(131072).expect("Vmo::create failed");
        vmo.write(b"hello", 5).expect("vmo.write failed");
        // SAFETY: Test code, only attach once, no other mappings.
        let vmo_id = unsafe { block_client.attach_vmo(&vmo) }.await.expect("attach_vmo failed");
        block_client
            .write_at(BufferSlice::new_with_vmo_id(&vmo_id, 0, 1024), 0)
            .await
            .expect("write_at failed");
        block_client
            .read_at(MutableBufferSlice::new_with_vmo_id(&vmo_id, 1024, 2048), 0)
            .await
            .expect("read_at failed");
        let mut buf: [u8; 5] = Default::default();
        vmo.read(&mut buf, 1029).expect("vmo.read failed");
        assert_eq!(&buf, b"hello");
        block_client.detach_vmo(vmo_id).await.expect("detach_vmo failed");
    }

    #[fuchsia::test]
    async fn test_alignment() {
        let (_ramdisk, _block_proxy, block_client) = make_ramdisk().await;
        let vmo = zx::Vmo::create(131072).expect("Vmo::create failed");
        // SAFETY: Test code, only attach once, no other mappings.
        let vmo_id = unsafe { block_client.attach_vmo(&vmo) }.await.expect("attach_vmo failed");
        block_client
            .write_at(BufferSlice::new_with_vmo_id(&vmo_id, 0, 1024), 1)
            .await
            .expect_err("expected failure due to bad alignment");
        block_client.detach_vmo(vmo_id).await.expect("detach_vmo failed");
    }

    #[fuchsia::test]
    async fn test_parallel_io() {
        let (_ramdisk, _block_proxy, block_client) = make_ramdisk().await;
        let vmo = zx::Vmo::create(131072).expect("Vmo::create failed");
        // SAFETY: Test code, only attach once, no other mappings.
        let vmo_id = unsafe { block_client.attach_vmo(&vmo) }.await.expect("attach_vmo failed");
        let mut reads = Vec::new();
        for _ in 0..1024 {
            reads.push(
                block_client
                    .read_at(MutableBufferSlice::new_with_vmo_id(&vmo_id, 0, 1024), 0)
                    .inspect_err(|e| panic!("read should have succeeded: {}", e)),
            );
        }
        futures::future::join_all(reads).await;
        block_client.detach_vmo(vmo_id).await.expect("detach_vmo failed");
    }

    #[fuchsia::test]
    async fn test_closed_device() {
        let (ramdisk, _block_proxy, block_client) = make_ramdisk().await;
        let vmo = zx::Vmo::create(131072).expect("Vmo::create failed");
        // SAFETY: Test code, only attach once, no other mappings.
        let vmo_id = unsafe { block_client.attach_vmo(&vmo) }.await.expect("attach_vmo failed");
        let mut reads = Vec::new();
        for _ in 0..1024 {
            reads.push(
                block_client.read_at(MutableBufferSlice::new_with_vmo_id(&vmo_id, 0, 1024), 0),
            );
        }
        assert!(block_client.is_connected());
        let _ = futures::join!(futures::future::join_all(reads), async {
            std::mem::drop(ramdisk);
        });
        // Destroying the ramdisk is asynchronous. Keep issuing reads until they start failing.
        while block_client
            .read_at(MutableBufferSlice::new_with_vmo_id(&vmo_id, 0, 1024), 0)
            .await
            .is_ok()
        {}

        // Sometimes the FIFO will start rejecting requests before FIFO is actually closed, so we
        // get false-positives from is_connected.
        while block_client.is_connected() {
            // Sleep for a bit to minimise lock contention.
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(500),
            ))
            .await;
        }

        // But once is_connected goes negative, it should stay negative.
        assert_eq!(block_client.is_connected(), false);
        let _ = block_client.detach_vmo(vmo_id).await;
    }

    #[fuchsia::test]
    async fn test_cancelled_reads() {
        let (_ramdisk, _block_proxy, block_client) = make_ramdisk().await;
        let vmo = zx::Vmo::create(131072).expect("Vmo::create failed");
        // SAFETY: Test code, only attach once, no other mappings.
        let vmo_id = unsafe { block_client.attach_vmo(&vmo) }.await.expect("attach_vmo failed");
        {
            let mut reads = FuturesUnordered::new();
            for _ in 0..1024 {
                reads.push(
                    block_client.read_at(MutableBufferSlice::new_with_vmo_id(&vmo_id, 0, 1024), 0),
                );
            }
            // Read the first 500 results and then dump the rest.
            for _ in 0..500 {
                reads.next().await;
            }
        }

        // Since we dropped pending futures (cancellation), the client must forcibly
        // terminate the connection to prevent any future unsafe I/O. Thus, further
        // calls like `detach_vmo` must fail with CANCELED.
        assert_eq!(
            block_client.read_at(MutableBufferSlice::new_with_vmo_id(&vmo_id, 0, 1024), 0).await,
            Err(zx::Status::CANCELED)
        );
        assert_eq!(block_client.detach_vmo(vmo_id).await, Err(zx::Status::CANCELED));
    }

    #[fuchsia::test]
    async fn test_parallel_large_read_and_write_with_memory_succeds() {
        let (_ramdisk, _block_proxy, block_client) = make_ramdisk().await;
        let block_client_ref = &block_client;
        let test_one = |offset, len, fill| async move {
            let buf = vec![fill; len];
            block_client_ref.write_at(buf[..].into(), offset).await.expect("write_at failed");
            // Read back an extra block either side.
            let mut read_buf = vec![0u8; len + 2 * RAMDISK_BLOCK_SIZE as usize];
            block_client_ref
                .read_at(read_buf.as_mut_slice().into(), offset - RAMDISK_BLOCK_SIZE)
                .await
                .expect("read_at failed");
            assert_eq!(
                &read_buf[0..RAMDISK_BLOCK_SIZE as usize],
                &[0; RAMDISK_BLOCK_SIZE as usize][..]
            );
            assert_eq!(
                &read_buf[RAMDISK_BLOCK_SIZE as usize..RAMDISK_BLOCK_SIZE as usize + len],
                &buf[..]
            );
            assert_eq!(
                &read_buf[RAMDISK_BLOCK_SIZE as usize + len..],
                &[0; RAMDISK_BLOCK_SIZE as usize][..]
            );
        };
        const WRITE_LEN: usize = super::TEMP_VMO_SIZE * 3 + RAMDISK_BLOCK_SIZE as usize;
        join!(
            test_one(RAMDISK_BLOCK_SIZE, WRITE_LEN, 0xa3u8),
            test_one(2 * RAMDISK_BLOCK_SIZE + WRITE_LEN as u64, WRITE_LEN, 0x7fu8)
        );
    }

    // Implements dummy server which can be used by test cases to verify whether
    // channel messages and fifo operations are being received - by using set_channel_handler or
    // set_fifo_hander respectively
    struct FakeBlockServer<'a> {
        server_channel: Option<fidl::endpoints::ServerEnd<block::BlockMarker>>,
        channel_handler: Box<dyn Fn(&block::SessionRequest) -> bool + 'a>,
        fifo_handler: Box<dyn Fn(BlockFifoRequest) -> BlockFifoResponse + 'a>,
    }

    impl<'a> FakeBlockServer<'a> {
        // Creates a new FakeBlockServer given a channel to listen on.
        //
        // 'channel_handler' and 'fifo_handler' closures allow for customizing the way how the server
        // handles requests received from channel or the fifo respectfully.
        //
        // 'channel_handler' receives a message before it is handled by the default implementation
        // and can return 'true' to indicate all processing is done and no further processing of
        // that message is required
        //
        // 'fifo_handler' takes as input a BlockFifoRequest and produces a response which the
        // FakeBlockServer will send over the fifo.
        fn new(
            server_channel: fidl::endpoints::ServerEnd<block::BlockMarker>,
            channel_handler: impl Fn(&block::SessionRequest) -> bool + 'a,
            fifo_handler: impl Fn(BlockFifoRequest) -> BlockFifoResponse + 'a,
        ) -> FakeBlockServer<'a> {
            FakeBlockServer {
                server_channel: Some(server_channel),
                channel_handler: Box::new(channel_handler),
                fifo_handler: Box::new(fifo_handler),
            }
        }

        // Runs the server
        async fn run(&mut self) {
            let server = self.server_channel.take().unwrap();

            // Set up a mock server.
            let (server_fifo, client_fifo) =
                zx::Fifo::<BlockFifoRequest, BlockFifoResponse>::create(16)
                    .expect("Fifo::create failed");
            let maybe_server_fifo = fuchsia_sync::Mutex::new(Some(client_fifo));

            let (fifo_future_abort, fifo_future_abort_registration) = AbortHandle::new_pair();
            let fifo_future = Abortable::new(
                async {
                    let mut fifo = fasync::Fifo::from_fifo(server_fifo);
                    let (mut reader, mut writer) = fifo.async_io();
                    let mut request = BlockFifoRequest::default();
                    loop {
                        match reader.read_entries(&mut request).await {
                            Ok(n) if n.get() == 1 => {}
                            Err(zx::Status::PEER_CLOSED) => break,
                            Err(e) => panic!("read_entry failed {:?}", e),
                            _ => unreachable!(),
                        };

                        let response = self.fifo_handler.as_ref()(request);
                        writer
                            .write_entries(std::slice::from_ref(&response))
                            .await
                            .expect("write_entries failed");
                    }
                },
                fifo_future_abort_registration,
            );

            let channel_future = async {
                server
                    .into_stream()
                    .for_each_concurrent(None, |request| async {
                        let request = request.expect("unexpected fidl error");

                        match request {
                            block::BlockRequest::GetInfo { responder } => {
                                responder
                                    .send(Ok(&block::BlockInfo {
                                        block_count: 1024,
                                        block_size: 512,
                                        max_transfer_size: 1024 * 1024,
                                        flags: block::DeviceFlag::empty(),
                                    }))
                                    .expect("send failed");
                            }
                            block::BlockRequest::OpenSession { session, control_handle: _ } => {
                                let stream = session.into_stream();
                                stream
                                    .for_each(|request| async {
                                        let request = request.expect("unexpected fidl error");
                                        // Give a chance for the test to register and potentially
                                        // handle the event
                                        if self.channel_handler.as_ref()(&request) {
                                            return;
                                        }
                                        match request {
                                            block::SessionRequest::GetFifo { responder } => {
                                                match maybe_server_fifo.lock().take() {
                                                    Some(fifo) => {
                                                        responder.send(Ok(fifo.downcast()))
                                                    }
                                                    None => responder.send(Err(
                                                        zx::Status::NO_RESOURCES.into_raw(),
                                                    )),
                                                }
                                                .expect("send failed")
                                            }
                                            block::SessionRequest::AttachVmo {
                                                vmo: _,
                                                responder,
                                            } => responder
                                                .send(Ok(&block::VmoId { id: 1 }))
                                                .expect("send failed"),
                                            block::SessionRequest::Close { responder } => {
                                                fifo_future_abort.abort();
                                                responder.send(Ok(())).expect("send failed")
                                            }
                                        }
                                    })
                                    .await
                            }
                            _ => panic!("Unexpected message"),
                        }
                    })
                    .await;
            };

            let _result = join!(fifo_future, channel_future);
            //_result can be Err(Aborted) since FifoClose calls .abort but that's expected
        }
    }

    #[fuchsia::test]
    async fn test_block_close_is_called() {
        let close_called = fuchsia_sync::Mutex::new(false);
        let (client_end, server) = fidl::endpoints::create_endpoints::<block::BlockMarker>();

        std::thread::spawn(move || {
            let _block_client =
                RemoteBlockClientSync::new(client_end).expect("RemoteBlockClientSync::new failed");
            // The drop here should cause Close to be sent.
        });

        let channel_handler = |request: &block::SessionRequest| -> bool {
            if let block::SessionRequest::Close { .. } = request {
                *close_called.lock() = true;
            }
            false
        };
        FakeBlockServer::new(server, channel_handler, |_| unreachable!()).run().await;

        // After the server has finished running, we can check to see that close was called.
        assert!(*close_called.lock());
    }

    #[fuchsia::test]
    async fn test_block_flush_is_called() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<block::BlockMarker>();

        struct Interface {
            flush_called: Arc<AtomicBool>,
        }
        impl block_server::async_interface::Interface for Interface {
            fn get_info(&self) -> Cow<'_, DeviceInfo> {
                Cow::Owned(DeviceInfo::Partition(PartitionInfo {
                    device_flags: fidl_fuchsia_storage_block::DeviceFlag::empty(),
                    max_transfer_blocks: None,
                    start_block_offset: None,
                    block_count: 1000,
                    type_guid: [0; 16],
                    instance_guid: [0; 16],
                    name: "foo".to_string(),
                    ..Default::default()
                }))
            }

            async fn read(
                &self,
                _device_block_offset: u64,
                _block_count: u32,
                _vmo: &Arc<zx::Vmo>,
                _vmo_offset: u64,
                _opts: ReadOptions,
                _trace_flow_id: Option<NonZero<u64>>,
            ) -> Result<(), zx::Status> {
                unreachable!();
            }

            async fn write(
                &self,
                _device_block_offset: u64,
                _block_count: u32,
                _vmo: &Arc<zx::Vmo>,
                _vmo_offset: u64,
                _opts: WriteOptions,
                _trace_flow_id: Option<NonZero<u64>>,
            ) -> Result<(), zx::Status> {
                unreachable!();
            }

            async fn flush(&self, _trace_flow_id: Option<NonZero<u64>>) -> Result<(), zx::Status> {
                self.flush_called.store(true, Ordering::Relaxed);
                Ok(())
            }

            async fn trim(
                &self,
                _device_block_offset: u64,
                _block_count: u32,
                _trace_flow_id: Option<NonZero<u64>>,
            ) -> Result<(), zx::Status> {
                unreachable!();
            }
        }

        let flush_called = Arc::new(AtomicBool::new(false));

        futures::join!(
            async {
                let block_client = RemoteBlockClient::new(proxy).await.expect("new failed");

                block_client.flush().await.expect("flush failed");
            },
            async {
                let block_server = BlockServer::new(
                    512,
                    Arc::new(Interface { flush_called: flush_called.clone() }),
                );
                block_server.handle_requests(stream.cast_stream()).await.unwrap();
            }
        );

        assert!(flush_called.load(Ordering::Relaxed));
    }

    #[fuchsia::test]
    async fn test_trace_flow_ids_set() {
        let (proxy, server) = fidl::endpoints::create_proxy();

        futures::join!(
            async {
                let block_client = RemoteBlockClient::new(proxy).await.expect("new failed");
                block_client.flush().await.expect("flush failed");
            },
            async {
                let flow_id: fuchsia_sync::Mutex<Option<u64>> = fuchsia_sync::Mutex::new(None);
                let fifo_handler = |request: BlockFifoRequest| -> BlockFifoResponse {
                    if request.trace_flow_id > 0 {
                        *flow_id.lock() = Some(request.trace_flow_id);
                    }
                    BlockFifoResponse {
                        status: zx::sys::ZX_OK,
                        reqid: request.reqid,
                        ..Default::default()
                    }
                };
                FakeBlockServer::new(server, |_| false, fifo_handler).run().await;
                // After the server has finished running, verify the trace flow ID was set to some value.
                assert!(flow_id.lock().is_some());
            }
        );
    }
}
