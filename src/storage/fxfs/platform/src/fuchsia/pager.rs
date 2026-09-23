// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::node::{FxNode, OpenedNode};
use crate::fuchsia::profile::Recorder;
use anyhow::Error;
use bitflags::bitflags;
use fuchsia_async::epoch::{Epoch, EpochGuard};
use fuchsia_async::{self as fasync};
use fuchsia_sync::{Mutex, MutexGuard};
use fxfs::future_with_guard::FutureWithGuard;
use fxfs::log::*;
use fxfs::range::RangeExt;
use fxfs::round::{round_down, round_up};
use std::future::Future;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use storage_device::buffer;
use storage_units::PAGE_SIZE;
use vfs::execution_scope::ExecutionScope;
use zx::sys::zx_page_request_command_t::{ZX_PAGER_VMO_DIRTY, ZX_PAGER_VMO_READ};
use zx::{PacketContents, PagerPacket, SignalPacket};

pub static STRONG_FILE_REFS: AtomicU64 = AtomicU64::new(0);

fn watch_for_zero_children(file: &impl PagerBacked) -> Result<(), zx::Status> {
    file.vmo().wait_async(
        file.pager().executor.port(),
        file.pager_packet_receiver_registration().key(),
        zx::Signals::VMO_ZERO_CHILDREN,
        zx::WaitAsyncOpts::empty(),
    )
}

pub type PagerPacketReceiverRegistration<T> = fasync::ReceiverRegistration<PagerPacketReceiver<T>>;

/// A `fuchsia_async::PacketReceiver` that handles pager packets and the `VMO_ZERO_CHILDREN` signal.
pub struct PagerPacketReceiver<T> {
    file: Mutex<FileHolder<T>>,
}

/// A returnable lock held on the receiver.
pub struct PagerPacketReceiverLock<'a, T> {
    _guard: MutexGuard<'a, FileHolder<T>>,
    strong: bool,
}

impl<T> PagerPacketReceiverLock<'_, T> {
    /// Returns true if the receiver was installed as a strong.
    pub fn is_strong(&self) -> bool {
        self.strong
    }
}

impl<T: PagerBacked> PagerPacketReceiver<T> {
    /// Drops the strong reference to the file that might be held if
    /// `Pager::watch_for_zero_children` was called. This should only be used when forcibly dropping
    /// the file object. Calls `on_zero_children` if the strong reference was held.
    pub fn stop_watching_for_zero_children(&self) {
        let mut file = self.file.lock();
        if let FileHolder::Strong(strong) = &*file {
            let weak = FileHolder::Weak(Arc::downgrade(&strong));
            let FileHolder::Strong(strong) = std::mem::replace(&mut *file, weak) else {
                unreachable!();
            };
            STRONG_FILE_REFS.fetch_sub(1, Ordering::Relaxed);
            strong.on_zero_children();
        }
    }

    /// Sets the current receiver and returns the lock guard so that it can be held after the value
    /// is set. Currently this allows synchronizing open count adjustments.
    pub fn set_receiver(&self, new_receiver: &Arc<T>) -> PagerPacketReceiverLock<'_, T> {
        let mut receiver_lock = self.file.lock();
        let strong = match &mut *receiver_lock {
            FileHolder::Strong(arc) => {
                *arc = new_receiver.clone();
                true
            }
            FileHolder::Weak(arc) => {
                *arc = Arc::downgrade(new_receiver);
                false
            }
        };
        PagerPacketReceiverLock { _guard: receiver_lock, strong }
    }

    fn receive_pager_packet(&self, contents: PagerPacket) {
        let command = contents.command();
        if command != ZX_PAGER_VMO_READ && command != ZX_PAGER_VMO_DIRTY {
            return;
        }

        let (opened_file, epoch_guard) = {
            let file_lock = self.file.lock();
            let file = match &*file_lock {
                FileHolder::Strong(file) => file.clone(),
                FileHolder::Weak(file) => {
                    if let Some(file) = file.upgrade() {
                        file
                    } else {
                        error!("Received a page request for a file that is closed {:?}", contents);
                        return;
                    }
                }
            };

            // Whenever a file is flushed, we must make sure existing page requests for a file are
            // completed to eliminate the possibility of supplying stale data for a file.  We solve
            // this by using a barrier when we flush to wait for outstanding page requests to
            // finish.  Technically, we only need to wait for page requests for the specific file
            // being flushed, but we should see if we need to for performance reasons first.
            let epoch_guard = match command {
                // Don't take refs for mark_dirty, it can block on flushes which block on the epoch
                // creating a deadlock. The call for awaiting epochs is `page_in_barrier` which
                // correctly implies that it should only wait on page in.
                ZX_PAGER_VMO_READ => Some(Epoch::global().guard()),
                _ => None,
            };

            // This needs to be done while holding the page lock so that it cannot race with moving
            // the open count on blob overwrite.
            let opened_file = match file.try_keep_open() {
                Ok(opened) => opened,
                Err(bare) => {
                    bare.pager().report_failure(
                        bare.vmo(),
                        contents.range(),
                        zx::Status::BAD_STATE,
                    );
                    return;
                }
            };
            (opened_file, epoch_guard)
        };

        // The scope guard needs to be held and outlive the file Arc and the clones of it.
        let Some(_scope_guard) = opened_file.pager().scope.try_active_guard() else {
            // If an active guard can't be acquired then the filesystem must be shutting down. Fail
            // the page request to avoid leaving the client hanging.
            opened_file.pager().report_failure(
                opened_file.vmo(),
                contents.range(),
                zx::Status::BAD_STATE,
            );
            return;
        };
        match command {
            ZX_PAGER_VMO_READ => {
                let file_arc = opened_file.clone();
                file_arc.page_in(PageInRange::new(
                    contents.range(),
                    opened_file,
                    epoch_guard.unwrap(),
                ))
            }
            ZX_PAGER_VMO_DIRTY => {
                let file_arc = opened_file.clone();
                file_arc.mark_dirty(MarkDirtyRange::new(contents.range(), opened_file))
            }
            _ => unreachable!("Unhandled commands are filtered above"),
        }
    }

    fn receive_signal_packet(&self, signals: SignalPacket) {
        assert!(signals.observed().contains(zx::Signals::VMO_ZERO_CHILDREN));

        // Check to see if there really are no children (which is necessary to avoid races) and, if
        // so, replace the strong reference with a weak one and call on_zero_children on the node.
        // If the file does have children, this asks the kernel to send us the ON_ZERO_CHILDREN
        // notification for the file.
        let mut file = self.file.lock();
        if let FileHolder::Strong(strong) = &*file {
            // If the last strong reference to the Arc is dropped here, then FxVolume's shutdown
            // won't wait for the inner node object to be dropped. Taking an active guard around
            // dropping the strong reference forces the FxVolume to wait for the file to be dropped.
            // If the scope has begun shutdown then we can't take an active guard, so instead we do
            // nothing here and the strong reference in the FileHolder will be removed by calling
            // `FxNode.terminate()` as part of `NodeCache.terminate()` in the FxVolume termination
            // thread.
            let Some(_guard) = strong.pager().scope.try_active_guard() else {
                info!("Ignoring zero-children notification due to shutting down");
                return;
            };
            match strong.vmo().info() {
                Ok(info) => {
                    if info.num_children == 0 {
                        let weak = FileHolder::Weak(Arc::downgrade(&strong));
                        let FileHolder::Strong(strong) = std::mem::replace(&mut *file, weak) else {
                            unreachable!();
                        };
                        STRONG_FILE_REFS.fetch_sub(1, Ordering::Relaxed);
                        strong.on_zero_children();
                    } else {
                        // There's not much we can do here if this fails, so we panic.
                        watch_for_zero_children(strong.as_ref()).unwrap();
                    }
                }
                Err(e) => error!(error:? = e; "Vmo::info failed"),
            }
        }
    }
}

impl<T: PagerBacked> fasync::PacketReceiver for PagerPacketReceiver<T> {
    fn receive_packet(&self, packet: zx::Packet) {
        match packet.contents() {
            PacketContents::Pager(contents) => {
                self.receive_pager_packet(contents);
            }
            PacketContents::SignalOne(signals) => {
                self.receive_signal_packet(signals);
            }
            _ => unreachable!(), // We don't expect any other kinds of packets.
        }
    }
}

pub struct Pager {
    pager: zx::Pager,
    scope: ExecutionScope,
    executor: fasync::EHandle,
    recorder: Mutex<Option<Box<dyn Recorder>>>,
}

// FileHolder is used to retain either a strong or a weak reference to a file.  If there are any
// child VMOs that have been shared, then we will have a strong reference which is required to keep
// the file alive.  When we detect that there are no more children, we can downgrade to a weak
// reference which will allow the file to be cleaned up if there are no other uses.
enum FileHolder<T> {
    Strong(Arc<T>),
    Weak(Weak<T>),
}

/// Pager handles page requests. It is a per-volume object.
impl Pager {
    /// Creates a new pager.
    pub fn new(scope: ExecutionScope) -> Result<Self, Error> {
        Ok(Pager {
            pager: zx::Pager::create(zx::PagerOptions::empty())?,
            scope,
            executor: fasync::EHandle::local(),
            recorder: Mutex::new(None),
        })
    }

    /// Spawns a short term task for the pager that includes a guard that will prevent termination.
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        if let Some(guard) = self.scope.try_active_guard() {
            self.executor.spawn_detached(FutureWithGuard::new(guard, task));
        }
    }

    /// Set the current profile recorder, or set to None to not record.
    pub fn set_recorder(&self, recorder: Option<Box<dyn Recorder>>) {
        // Drop the old one outside of the lock.
        let _old = std::mem::replace(&mut (*self.recorder.lock()), recorder);
    }

    /// Borrow the profile recorder. Used to record file opens.
    pub fn recorder(&self) -> MutexGuard<'_, Option<Box<dyn Recorder>>> {
        self.recorder.lock()
    }

    /// Record a range into a profile if one is being recorded.
    pub fn record_page_in<P: PagerBacked>(&self, node: Arc<P>, range: Range<u64>) {
        let mut recorder_holder = self.recorder.lock();
        if let Some(recorder) = &mut (*recorder_holder) {
            // If the message fails to send, so will all the rest.
            if let Err(_) = recorder.record(node, range.start) {
                *recorder_holder = None;
            }
        }
    }

    /// Creates a new VMO to be used with the pager.
    pub fn create_vmo<T: PagerBacked>(
        &self,
        file: Weak<T>,
        initial_size: u64,
        vmo_options: zx::VmoOptions,
    ) -> Result<(zx::Vmo, PagerPacketReceiverRegistration<T>), Error> {
        let registration = self
            .executor
            .register_receiver(PagerPacketReceiver { file: Mutex::new(FileHolder::Weak(file)) });
        Ok((
            self.pager.create_vmo(
                vmo_options,
                self.executor.port(),
                registration.key(),
                initial_size,
            )?,
            registration,
        ))
    }

    /// Starts watching for the `VMO_ZERO_CHILDREN` signal on `file`'s vmo. Returns false if the
    /// signal is already being watched for. When the pager receives the `VMO_ZERO_CHILDREN` signal
    /// [`PagerBacked::on_zero_children`] will be called.
    pub fn watch_for_zero_children(&self, file: &impl PagerBacked) -> Result<bool, Error> {
        let mut file = file.pager_packet_receiver_registration().file.lock();

        match &*file {
            FileHolder::Weak(weak) => {
                // Should never fail because watch_for_zero_children should be called from `file`.
                let strong = weak.upgrade().unwrap();

                watch_for_zero_children(strong.as_ref())?;

                STRONG_FILE_REFS.fetch_add(1, Ordering::Relaxed);
                *file = FileHolder::Strong(strong);
                Ok(true)
            }
            FileHolder::Strong(_) => Ok(false),
        }
    }

    /// Supplies pages in response to a `ZX_PAGER_VMO_READ` page request. See
    /// `zx_pager_supply_pages` for more information.
    fn supply_pages(
        &self,
        vmo: &zx::Vmo,
        range: Range<u64>,
        transfer_vmo: &zx::Vmo,
        transfer_offset: u64,
    ) {
        if let Err(e) = self.pager.supply_pages(vmo, range, transfer_vmo, transfer_offset) {
            error!(error:? = e; "supply_pages failed");
        }
    }

    /// Notifies the kernel that a page request for the given `range` has failed. Sent in response
    /// to a `ZX_PAGER_VMO_READ` or `ZX_PAGER_VMO_DIRTY` page request. See `ZX_PAGER_OP_FAIL` for
    /// more information.
    fn report_failure(&self, vmo: &zx::Vmo, range: Range<u64>, status: zx::Status) {
        let pager_status = match status {
            zx::Status::IO_DATA_INTEGRITY => zx::Status::IO_DATA_INTEGRITY,
            zx::Status::NO_SPACE => zx::Status::NO_SPACE,
            zx::Status::FILE_BIG => zx::Status::BUFFER_TOO_SMALL,
            zx::Status::IO
            | zx::Status::IO_DATA_LOSS
            | zx::Status::IO_INVALID
            | zx::Status::IO_MISSED_DEADLINE
            | zx::Status::IO_NOT_PRESENT
            | zx::Status::IO_OVERRUN
            | zx::Status::IO_REFUSED
            | zx::Status::PEER_CLOSED => zx::Status::IO,
            _ => zx::Status::BAD_STATE,
        };
        if let Err(e) = self.pager.op_range(zx::PagerOp::Fail(pager_status), vmo, range) {
            error!(error:? = e; "op_range failed");
        }
    }

    /// Allows the kernel to dirty the `range` of pages. Sent in response to a `ZX_PAGER_VMO_DIRTY`
    /// page request. See `ZX_PAGER_OP_DIRTY` for more information.
    fn dirty_pages(&self, vmo: &zx::Vmo, range: Range<u64>) -> Result<(), zx::Status> {
        self.pager.op_range(zx::PagerOp::Dirty, vmo, range).inspect_err(|error| {
            // It is possible for `ZX_ERR_NOT_FOUND` to be returned on a clean page that has
            // been evicted.  In this case, the kernel will retry if necessary.  See
            // https://fxbug.dev/42086069 for more information.
            if *error != zx::Status::NOT_FOUND {
                error!(error:?; "dirty_pages failed");
            }
        })
    }

    /// Notifies the kernel that the filesystem has started cleaning the `range` of pages. See
    /// `ZX_PAGER_OP_WRITEBACK_BEGIN` for more information.
    pub fn writeback_begin(
        &self,
        vmo: &zx::Vmo,
        range: Range<u64>,
        options: zx::PagerWritebackBeginOptions,
    ) {
        if let Err(e) = self.pager.op_range(zx::PagerOp::WritebackBegin(options), vmo, range) {
            error!(error:? = e; "writeback_begin failed");
        }
    }

    /// Notifies the kernel that the filesystem has finished cleaning the `range` of pages. See
    /// `ZX_PAGER_OP_WRITEBACK_END` for more information.
    pub fn writeback_end(&self, vmo: &zx::Vmo, range: Range<u64>) {
        if let Err(e) = self.pager.op_range(zx::PagerOp::WritebackEnd, vmo, range) {
            error!(error:? = e; "writeback_end failed");
        }
    }

    /// Queries the `vmo` for ranges that are dirty within `range`. Returns `(num_returned,
    /// num_remaining)` where `num_returned` is the number of objects populated in `buffer` and
    /// `num_remaining` is the number of dirty ranges remaining in `range` that could not fit in
    /// `buffer`. See `zx_pager_query_dirty_ranges` for more information.
    pub fn query_dirty_ranges(
        &self,
        vmo: &zx::Vmo,
        range: Range<u64>,
        buffer: &mut [VmoDirtyRange],
    ) -> Result<(usize, usize), zx::Status> {
        let mut actual = 0;
        let mut avail = 0;
        let status = unsafe {
            // TODO(https://fxbug.dev/42142550) Move to src/lib/zircon/rust/src/pager.rs once
            // query_dirty_ranges is part of the stable vDSO.
            zx::sys::zx_pager_query_dirty_ranges(
                self.pager.raw_handle(),
                vmo.raw_handle(),
                range.start,
                range.end - range.start,
                buffer.as_mut_ptr() as *mut u8,
                std::mem::size_of_val(buffer),
                &mut actual as *mut usize,
                &mut avail as *mut usize,
            )
        };
        zx::ok(status).map(|_| (actual, avail - actual))
    }

    /// Queries the `vmo` for any pager related statistics. If
    /// `PagerVmoStatsOptions::RESET_VMO_STATS` is passed then the stats will also be reset. See
    /// `zx_pager_query_vmo_stats` for more information.
    pub fn query_vmo_stats(
        &self,
        vmo: &zx::Vmo,
        options: PagerVmoStatsOptions,
    ) -> Result<PagerVmoStats, zx::Status> {
        #[repr(C)]
        #[derive(Default)]
        struct zx_pager_vmo_stats {
            pub modified: u32,
        }
        const ZX_PAGER_VMO_STATS_MODIFIED: u32 = 1;
        let mut vmo_stats = MaybeUninit::<zx_pager_vmo_stats>::uninit();
        let status = unsafe {
            // TODO(https://fxbug.dev/42142550) Move to src/lib/zircon/rust/src/pager.rs once
            // query_vmo_stats is part of the stable vDSO.
            zx::sys::zx_pager_query_vmo_stats(
                self.pager.raw_handle(),
                vmo.raw_handle(),
                options.bits(),
                vmo_stats.as_mut_ptr() as *mut u8,
                std::mem::size_of::<zx_pager_vmo_stats>(),
            )
        };
        zx::ok(status)?;
        let vmo_stats = unsafe { vmo_stats.assume_init() };
        Ok(PagerVmoStats { was_vmo_modified: vmo_stats.modified == ZX_PAGER_VMO_STATS_MODIFIED })
    }

    pub async fn page_in_barrier() {
        Epoch::global().barrier().await;
    }
}

/// This is a trait for objects (files/blobs) that expose a pager backed VMO.
pub trait PagerBacked: FxNode + Sync + Send + Sized + 'static {
    /// Create an OpenedNode if it is already open, if not, return the bare Arc.
    fn try_keep_open(self: Arc<Self>) -> Result<OpenedNode<Self>, Arc<Self>>;

    /// The pager backing this VMO.
    fn pager(&self) -> &Pager;

    /// The receiver registration returned from [`Pager::create_vmo`].
    fn pager_packet_receiver_registration(&self) -> &PagerPacketReceiverRegistration<Self>;

    /// The pager backed VMO that this object is handling packets for. The VMO must be created with
    /// [`Pager::create_vmo`].
    fn vmo(&self) -> &zx::Vmo;

    /// Called by the pager when a `ZX_PAGER_VMO_READ` packet is received for the VMO. The
    /// implementation must respond by calling either `PageInRange::supply_pages` or
    /// `PageInRange::report_failure`.
    fn page_in(self: Arc<Self>, range: PageInRange<Self>);

    /// Called by the pager when a `ZX_PAGER_VMO_DIRTY` packet is received for the VMO. The
    /// implementation must respond by calling either `MarkDirtyRange::dirty_pages` or
    /// `MarkDirtyRange::report_failure`.
    fn mark_dirty(self: Arc<Self>, range: MarkDirtyRange<Self>);

    /// Called by the pager to indicate there are no more VMO children.
    fn on_zero_children(self: Arc<Self>);

    /// Total bytes readable. Anything reads over this will be zero padded in the VMO.
    fn byte_size(&self) -> u64;

    /// Reads one or more blocks into a buffer and returns it. This method is called by
    /// `default_page_in` and `aligned_byte_range` will always be aligned to the `read_ahead_size`
    /// past to `default_page_in` unless that would extend beyond `self.byte_size()`, in which case,
    /// `aligned_byte_range` will end at `self.byte_size()`'s next page multiple. The returned
    /// buffer must be at least as large as the requested range. Only the requested range will be
    /// supplied to the pager.
    fn aligned_read(
        &self,
        aligned_byte_range: std::ops::Range<u64>,
    ) -> impl Future<Output = Result<buffer::Buffer<'_>, Error>> + Send;
}

/// A generic page_in implementation that supplies pages using block-aligned reads.
pub fn default_page_in<P: PagerBacked>(
    this: Arc<P>,
    pager_range: PageInRange<P>,
    read_ahead_size: u64,
) {
    fxfs_trace::duration!(
        "start-page-in",
        "offset" => pager_range.start(),
        "len" => pager_range.len()
    );

    const ZERO_VMO_SIZE: u64 = 1_048_576;
    static ZERO_VMO: std::sync::LazyLock<zx::Vmo> =
        std::sync::LazyLock::new(|| zx::Vmo::create(ZERO_VMO_SIZE).unwrap());

    assert!(pager_range.end() < i64::MAX as u64);

    // Two important subtleties to consider in this space:
    //
    // `byte_size` is the official size of the object. VMOs are page-aligned so `page_aligned_size`
    // is the "official" page length of the object. This may be smaller than Vmo::get_size because
    // these two things are not updated atomically. The reverse is not true -- We do not currently
    // ever shrink a VMO's size. We also do not update byte_size (self.handle.get_size()) if an
    // independent handle is used to grow a file. This means the VMO's size should always be
    // strictly equal or bigger than `byte_size`.
    //
    // It is valid to supply more pages than asked, but supplying pages outside of the VMO range
    // will trigger OUT_OF_RANGE errors and the call will fail without supplying anything. We must
    // supply the range requested under all circumstances to unblock any page misses but we should
    // take care to never supply additional pages beyond `page_aligned_size` as there is a chance
    // that we might serve a range outside of the VMO and fail to supply anything at all.

    let page_aligned_size = PAGE_SIZE.align_up(this.byte_size()).unwrap();

    // Zero-pad the tail if the requested range exceeds the size of the thing we're reading. This
    // can happen when we truncate and there are outstanding pager requests that the kernel was not
    // able to cancel in time.
    let (read_range, zero_range) = pager_range.split(page_aligned_size);
    if let Some(zero_range) = zero_range {
        for range in zero_range.chunks(ZERO_VMO_SIZE) {
            range.supply_pages(&ZERO_VMO, 0);
        }
    }

    if let Some(read_range) = read_range {
        let expanded_range_for_readahead = round_down(read_range.start(), read_ahead_size)
            ..std::cmp::min(
                round_up(read_range.end(), read_ahead_size).unwrap(),
                page_aligned_size,
            );
        let read_range = read_range.expand(expanded_range_for_readahead);
        for range in read_range.chunks(read_ahead_size) {
            // Record the page in before spawning the task to handle the page-in. This is necessary
            // so that we don't miss this page-in when replaying and recording a new profile.  The
            // replay is considered finished once we've responded to the page request, so if we if
            // we spawn the page request before recording the page-in, it's possible (albeit
            // unlikely) that the profiler can think the replay has finished, but not know about the
            // page request and so the next recording to be missing the page request.  With the
            // order swapped, the `test_profile` test would have a rare flake.
            this.pager().record_page_in(this.clone(), range.range.clone());

            this.pager().spawn(page_in_chunk(this.clone(), range));
        }
    }
}

#[fxfs_trace::trace("offset" => read_range.start(), "len" => read_range.len())]
async fn page_in_chunk<P: PagerBacked>(this: Arc<P>, read_range: PageInRange<P>) {
    let buffer = match this.aligned_read(read_range.range()).await {
        Ok(v) => v,
        Err(error) => {
            error!(range:? = read_range.range(), error:?; "Failed to load range");
            read_range.report_failure(map_to_status(error));
            return;
        }
    };
    assert!(
        buffer.len() as u64 >= read_range.len(),
        "A buffer smaller than requested was returned. requested: {}, returned: {}",
        read_range.len(),
        buffer.len()
    );
    read_range.supply_pages(buffer.allocator().buffer_source().vmo(), buffer.range().start as u64);
}

/// Represents a dirty range of page aligned bytes within a pager backed VMO.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct VmoDirtyRange {
    offset: u64,
    length: u64,
    options: u64,
}

impl VmoDirtyRange {
    /// The page aligned byte range.
    pub fn range(&self) -> Range<u64> {
        self.offset..(self.offset + self.length)
    }

    /// Returns true if all of the bytes in the range are 0.
    pub fn is_zero_range(&self) -> bool {
        self.options & zx::sys::ZX_VMO_DIRTY_RANGE_IS_ZERO != 0
    }
}

bitflags! {
    /// Options for `Pager::query_vmo_stats`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[repr(transparent)]
    pub struct PagerVmoStatsOptions: u32 {
        /// Resets the stats at the of the `Pager::query_vmo_stats` call.
        const RESET_VMO_STATS = 1;
    }
}

/// Pager related statistic for a VMO.
#[derive(Debug)]
pub struct PagerVmoStats {
    was_vmo_modified: bool,
}

impl PagerVmoStats {
    /// Returns true if the VMO was modified since the last time the VMO stats were reset.
    pub fn was_vmo_modified(&self) -> bool {
        self.was_vmo_modified
    }
}

/// A trait for specializing `PagerRange` for different request types.
pub trait PagerRequestType {
    /// Returns the name of the request type for logging purposes.
    fn request_type_name() -> &'static str;
}

/// A request generated from a ZX_PAGER_VMO_READ packet.
pub struct PageInRequest;

impl PagerRequestType for PageInRequest {
    fn request_type_name() -> &'static str {
        "PageInRequest"
    }
}

/// The requested range from a ZX_PAGER_VMO_READ packet. This object must not be dropped without
/// calling either `supply_pages` or `report_failure`.
pub type PageInRange<T> = PagerRange<T, PageInRequest>;

impl<T: PagerBacked> PageInRange<T> {
    /// Constructs a new `PageInRange<T>`. `range` must be page aligned.
    pub fn new(range: Range<u64>, file: OpenedNode<T>, epoch_guard: EpochGuard<'static>) -> Self {
        debug_assert!(PAGE_SIZE.is_aligned(&range), "{range:?} is not page aligned",);
        Self {
            range,
            inner: Some(PagerRangeInner { file, _epoch_guard: Some(epoch_guard) }),
            _request_type: PhantomData,
        }
    }

    /// Supplies pages to the kernel for this range. See `zx_pager_supply_pages` for more
    /// information.
    pub fn supply_pages(mut self, transfer_vmo: &zx::Vmo, transfer_offset: u64) {
        let inner = self.inner.take().unwrap();
        inner.file.pager().supply_pages(
            inner.file.vmo(),
            self.range.clone(),
            transfer_vmo,
            transfer_offset,
        );
    }
}

/// A requested generated from a ZX_PAGER_VMO_DIRTY packet.
#[derive(Debug)]
pub struct MarkDirtyRequest;

impl PagerRequestType for MarkDirtyRequest {
    fn request_type_name() -> &'static str {
        "MarkDirtyRequest"
    }
}

/// The requested range from a ZX_PAGER_VMO_DIRTY packet. This object must not be dropped without
/// calling either `mark_dirty` or `report_failure`.
pub type MarkDirtyRange<T> = PagerRange<T, MarkDirtyRequest>;

impl<T: PagerBacked> MarkDirtyRange<T> {
    /// Constructs a new `MarkDirtyRange<T>`. `range` must be page aligned.
    pub fn new(range: Range<u64>, file: OpenedNode<T>) -> Self {
        debug_assert!(PAGE_SIZE.is_aligned(&range), "{range:?} is not page aligned");
        Self {
            range,
            inner: Some(PagerRangeInner { file, _epoch_guard: None }),
            _request_type: PhantomData,
        }
    }

    /// Allows the kernel to dirty this range of pages. See `ZX_PAGER_OP_DIRTY` for more
    /// information.
    pub fn dirty_pages(mut self) -> Result<(), zx::Status> {
        let inner = self.inner.take().unwrap();
        inner.file.pager().dirty_pages(inner.file.vmo(), self.range.clone())
    }
}

struct PagerRangeInner<T: PagerBacked> {
    file: OpenedNode<T>,

    /// Holds a reference to the current Epoch, so that in-flight read requests can be tracked. This
    /// should be None for MarkDirty requests.
    _epoch_guard: Option<EpochGuard<'static>>,
}

impl<T: PagerBacked> Clone for PagerRangeInner<T> {
    fn clone(&self) -> Self {
        Self { file: self.file.dup(), _epoch_guard: self._epoch_guard.clone() }
    }
}

/// The requested range from a pager packet. This object ensures that all pager requests receive a
/// response.
pub struct PagerRange<T: PagerBacked, U: PagerRequestType> {
    range: Range<u64>,

    /// Contains the file and the ref guard. If this is None, then the request is complete.
    inner: Option<PagerRangeInner<T>>,

    _request_type: PhantomData<U>,
}

impl<T: PagerBacked, U: PagerRequestType> PagerRange<T, U> {
    pub fn file(&self) -> &OpenedNode<T> {
        &self.inner.as_ref().unwrap().file
    }

    /// Splits the underlying range allowing for different parts of the range to be handled and
    /// responded to independently. See `RangeExt::split` for how splitting a range works.
    /// `split_point` must be page aligned.
    pub fn split(mut self, split_point: u64) -> (Option<Self>, Option<Self>) {
        let inner = self.inner.take().unwrap();
        let (left, right) = self.range.clone().split(split_point);
        let right = right.map(|range| Self {
            range,
            inner: Some(inner.clone()),
            _request_type: PhantomData,
        });
        let left = left.map(|range| Self { range, inner: Some(inner), _request_type: PhantomData });
        (left, right)
    }

    /// Increases the size of the range that will be responded to. Panics if the current range is
    /// not a subset of `new_range`. `new_range` must be page aligned.
    pub fn expand(mut self, new_range: Range<u64>) -> Self {
        assert!(
            self.range.start >= new_range.start && self.range.end <= new_range.end,
            "{:?} is not a subset of {:?}",
            self.range,
            new_range
        );
        debug_assert!(PAGE_SIZE.is_aligned(&new_range), "{new_range:?} is not page aligned");
        self.range = new_range;
        self
    }

    /// Returns an iterator that splits the range into ranges of `chunk_size`. If the length of the
    /// range is not a multiple of `chunk_size` then the last chunk won't be of length `chunk_size`.
    /// The returned iterator will panic if it's dropped without being fully consumed. `chunk_size`
    /// must a multiple of the page size.
    pub fn chunks(mut self, chunk_size: u64) -> PagerRangeChunksIter<T, U> {
        debug_assert!(
            PAGE_SIZE.is_aligned(chunk_size),
            "{chunk_size} is not a multiple of the page size"
        );
        PagerRangeChunksIter {
            start: self.range.start,
            end: self.range.end,
            chunk_size: chunk_size,
            inner: self.inner.take(),
            _request_type: PhantomData,
        }
    }

    #[inline]
    pub fn start(&self) -> u64 {
        self.range.start
    }

    #[inline]
    pub fn end(&self) -> u64 {
        self.range.end
    }

    #[inline]
    pub fn len(&self) -> u64 {
        self.range.end - self.range.start
    }

    #[inline]
    pub fn range(&self) -> Range<u64> {
        self.range.clone()
    }

    /// Notifies the kernel that the page request for this range has failed. See `ZX_PAGER_OP_FAIL`
    /// for more information.
    pub fn report_failure(mut self, status: zx::Status) {
        let inner = self.inner.take().unwrap();
        inner.file.pager().report_failure(inner.file.vmo(), self.range.clone(), status);
    }

    /// Test only method that will consume the PagerRange without having the send a response.
    #[cfg(test)]
    fn consume(mut self) {
        self.inner.take().unwrap();
    }
}

impl<T: PagerBacked, U: PagerRequestType> Drop for PagerRange<T, U> {
    fn drop(&mut self) {
        if let Some(inner) = &self.inner {
            let request_type = U::request_type_name();
            let range = self.range.clone();
            let key = inner.file.pager_packet_receiver_registration().key();
            if cfg!(debug_assertions) {
                // If this object is being dropped as part of a panic then avoid panicking again.
                // Dropping pager packets when fxfs is crashing is acceptable. Panicking again would
                // only clutter the logs.
                if !std::thread::panicking() {
                    panic!(
                        "PagerRange was dropped without sending a response, \
                        request_type={request_type}, range={range:?}, key={key}",
                    );
                }
            } else {
                error!(
                    "PagerRange was dropped without sending a response, \
                    request_type={request_type}, range={range:?}, key={key}",
                );
                inner.file.pager().report_failure(inner.file.vmo(), range, zx::Status::BAD_STATE);
            }
        }
    }
}

/// An iterator similar to `std::slice::Chunks` which yields `PagerRange` objects.
/// `PagerRangeChunksIter` will panic if it's dropped without being fully consumed.
pub struct PagerRangeChunksIter<T: PagerBacked, U: PagerRequestType> {
    start: u64,
    end: u64,
    chunk_size: u64,
    /// The file and locks/references that need to survive the request.
    inner: Option<PagerRangeInner<T>>,
    _request_type: PhantomData<U>,
}

impl<T: PagerBacked, U: PagerRequestType> Iterator for PagerRangeChunksIter<T, U> {
    type Item = PagerRange<T, U>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.start == self.end {
            None
        } else if self.start + self.chunk_size >= self.end {
            let next = Self::Item {
                range: self.start..self.end,
                inner: self.inner.take(),
                _request_type: PhantomData,
            };
            self.start = self.end;
            Some(next)
        } else {
            let next_end = self.start + self.chunk_size;
            let next = Self::Item {
                range: self.start..next_end,
                inner: self.inner.clone(),
                _request_type: PhantomData,
            };
            self.start = next_end;
            Some(next)
        }
    }
}

impl<T: PagerBacked, U: PagerRequestType> Drop for PagerRangeChunksIter<T, U> {
    fn drop(&mut self) {
        if self.start != self.end {
            let request_type = U::request_type_name();
            let remaining = self.start..self.end;
            let inner = self.inner.take().unwrap();
            let key = inner.file.pager_packet_receiver_registration().key();
            if cfg!(debug_assertions) {
                // If this object is being dropped as part of a panic then avoid panicking again.
                // Dropping pager packets when fxfs is crashing is acceptable. Panicking again would
                // only clutter the logs.
                if !std::thread::panicking() {
                    panic!(
                        "PagerRangeChunksIter was dropped without being fully consumed, \
                    request_type={request_type}, remaining={remaining:?}, key={key}",
                    );
                }
            } else {
                error!(
                    "PagerRangeChunksIter was dropped without being fully consumed, \
                    request_type={request_type}, remaining={remaining:?}, key={key}",
                );
                inner.file.pager().report_failure(
                    inner.file.vmo(),
                    remaining,
                    zx::Status::BAD_STATE,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use futures::channel::mpsc;
    use fxfs_macros::ToWeakNode;

    fn page_size() -> u64 {
        PAGE_SIZE.get()
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum PagerRequest {
        PageIn(Range<u64>),
        Dirty(Range<u64>),
    }

    #[derive(ToWeakNode)]
    struct MockFile {
        vmo: zx::Vmo,
        pager_packet_receiver_registration: PagerPacketReceiverRegistration<Self>,
        pager: Arc<Pager>,
        /// page in requests get logged so we can compare actual calls to to expectations.
        pager_requests: Mutex<Vec<PagerRequest>>,
    }

    impl MockFile {
        fn new(pager: Arc<Pager>) -> Arc<Self> {
            Self::new_with_size_and_type(pager, page_size(), zx::VmoOptions::UNBOUNDED)
        }

        fn new_with_size_and_type(
            pager: Arc<Pager>,
            size: u64,
            vmo_type: zx::VmoOptions,
        ) -> Arc<Self> {
            Arc::new_cyclic(|weak| {
                let (vmo, pager_packet_receiver_registration) = pager
                    .create_vmo(weak.clone(), size, vmo_type | zx::VmoOptions::TRAP_DIRTY)
                    .unwrap();
                Self {
                    pager,
                    vmo,
                    pager_packet_receiver_registration,
                    pager_requests: Default::default(),
                }
            })
        }

        // Returns the page_in requests received for this file.
        fn pager_requests(&self, reset: bool) -> Vec<PagerRequest> {
            if reset {
                std::mem::take(&mut *self.pager_requests.lock())
            } else {
                self.pager_requests.lock().clone()
            }
        }
    }

    impl FxNode for MockFile {
        fn object_id(&self) -> u64 {
            unimplemented!();
        }

        fn parent(&self) -> Option<Arc<crate::directory::FxDirectory>> {
            unimplemented!();
        }

        fn set_parent(&self, _parent: Arc<crate::directory::FxDirectory>) {
            unimplemented!();
        }

        fn open_count_add_one(&self) {}

        fn open_count_sub_one(self: Arc<Self>) {}

        fn object_descriptor(&self) -> fxfs::object_store::ObjectDescriptor {
            unimplemented!();
        }
    }

    impl PagerBacked for MockFile {
        fn try_keep_open(self: Arc<Self>) -> Result<OpenedNode<Self>, Arc<Self>> {
            Ok(OpenedNode(self))
        }

        fn pager(&self) -> &Pager {
            &self.pager
        }

        fn pager_packet_receiver_registration(&self) -> &PagerPacketReceiverRegistration<Self> {
            &self.pager_packet_receiver_registration
        }

        fn vmo(&self) -> &zx::Vmo {
            &self.vmo
        }

        fn page_in(self: Arc<Self>, range: PageInRange<Self>) {
            let aux_vmo = zx::Vmo::create(range.len()).unwrap();
            self.pager_requests.lock().push(PagerRequest::PageIn(range.range()));
            range.supply_pages(&aux_vmo, 0);
        }

        fn mark_dirty(self: Arc<Self>, range: MarkDirtyRange<Self>) {
            self.pager_requests.lock().push(PagerRequest::Dirty(range.range()));
            let _ = range.dirty_pages();
        }

        fn on_zero_children(self: Arc<Self>) {}

        fn byte_size(&self) -> u64 {
            unimplemented!();
        }
        async fn aligned_read(
            &self,
            _aligned_byte_range: std::ops::Range<u64>,
        ) -> Result<buffer::Buffer<'_>, Error> {
            unimplemented!();
        }
    }

    #[derive(ToWeakNode)]
    struct OnZeroChildrenFile {
        pager: Arc<Pager>,
        vmo: zx::Vmo,
        pager_packet_receiver_registration: PagerPacketReceiverRegistration<Self>,
        sender: Mutex<mpsc::UnboundedSender<()>>,
    }

    impl OnZeroChildrenFile {
        fn new(pager: Arc<Pager>, sender: mpsc::UnboundedSender<()>) -> Arc<Self> {
            Arc::new_cyclic(|weak| {
                let (vmo, pager_packet_receiver_registration) =
                    pager.create_vmo(weak.clone(), page_size(), zx::VmoOptions::empty()).unwrap();
                Self { pager, vmo, pager_packet_receiver_registration, sender: Mutex::new(sender) }
            })
        }
    }

    impl FxNode for OnZeroChildrenFile {
        fn object_id(&self) -> u64 {
            unimplemented!();
        }

        fn parent(&self) -> Option<Arc<crate::directory::FxDirectory>> {
            unimplemented!();
        }

        fn set_parent(&self, _parent: Arc<crate::directory::FxDirectory>) {
            unimplemented!();
        }

        fn open_count_add_one(&self) {}

        fn open_count_sub_one(self: Arc<Self>) {}

        fn object_descriptor(&self) -> fxfs::object_store::ObjectDescriptor {
            unimplemented!();
        }
    }

    impl PagerBacked for OnZeroChildrenFile {
        fn try_keep_open(self: Arc<Self>) -> Result<OpenedNode<Self>, Arc<Self>> {
            Ok(OpenedNode(self))
        }

        fn pager(&self) -> &Pager {
            &self.pager
        }

        fn pager_packet_receiver_registration(&self) -> &PagerPacketReceiverRegistration<Self> {
            &self.pager_packet_receiver_registration
        }

        fn vmo(&self) -> &zx::Vmo {
            &self.vmo
        }

        fn page_in(self: Arc<Self>, _range: PageInRange<Self>) {
            unreachable!();
        }

        fn mark_dirty(self: Arc<Self>, _range: MarkDirtyRange<Self>) {
            unreachable!();
        }

        fn on_zero_children(self: Arc<Self>) {
            self.sender.lock().unbounded_send(()).unwrap();
        }
        fn byte_size(&self) -> u64 {
            unreachable!();
        }
        async fn aligned_read(
            &self,
            _aligned_byte_range: std::ops::Range<u64>,
        ) -> Result<buffer::Buffer<'_>, Error> {
            unreachable!();
        }
    }

    #[fuchsia::test(threads = 2)]
    async fn test_watch_for_zero_children() {
        let (sender, mut receiver) = mpsc::unbounded();
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let file = OnZeroChildrenFile::new(pager.clone(), sender);
        {
            let _child_vmo = file
                .vmo()
                .create_child(
                    zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE,
                    0,
                    file.vmo().get_content_size().unwrap(),
                )
                .unwrap();
            assert!(pager.watch_for_zero_children(file.as_ref()).unwrap());
        }
        // Wait for `on_zero_children` to be called.
        receiver.next().await.unwrap();

        scope.wait().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_multiple_watch_for_zero_children_calls() {
        let (sender, mut receiver) = mpsc::unbounded();
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let file = OnZeroChildrenFile::new(pager.clone(), sender);
        {
            let _child_vmo = file
                .vmo()
                .create_child(
                    zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE,
                    0,
                    file.vmo().get_content_size().unwrap(),
                )
                .unwrap();
            assert!(pager.watch_for_zero_children(file.as_ref()).unwrap());
            // `watch_for_zero_children` will return false when it's already watching.
            assert!(!pager.watch_for_zero_children(file.as_ref()).unwrap());
        }
        receiver.next().await.unwrap();

        // The pager stops listening for VMO_ZERO_CHILDREN once the signal fires. Calling
        // `watch_for_zero_children` afterwards should return true again because watching had
        // stopped.
        assert!(pager.watch_for_zero_children(file.as_ref()).unwrap());

        file.pager_packet_receiver_registration.stop_watching_for_zero_children();

        scope.wait().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_status_code_mapping() {
        #[derive(ToWeakNode)]
        struct StatusCodeFile {
            vmo: zx::Vmo,
            pager: Arc<Pager>,
            status_code: Mutex<zx::Status>,
            pager_packet_receiver_registration: PagerPacketReceiverRegistration<Self>,
        }

        impl FxNode for StatusCodeFile {
            fn object_id(&self) -> u64 {
                unimplemented!();
            }

            fn parent(&self) -> Option<Arc<crate::directory::FxDirectory>> {
                unimplemented!();
            }

            fn set_parent(&self, _parent: Arc<crate::directory::FxDirectory>) {
                unimplemented!();
            }

            fn open_count_add_one(&self) {}

            fn open_count_sub_one(self: Arc<Self>) {}

            fn object_descriptor(&self) -> fxfs::object_store::ObjectDescriptor {
                unimplemented!();
            }
        }

        impl PagerBacked for StatusCodeFile {
            fn try_keep_open(self: Arc<Self>) -> Result<OpenedNode<Self>, Arc<Self>> {
                Ok(OpenedNode(self))
            }

            fn pager(&self) -> &Pager {
                &self.pager
            }

            fn pager_packet_receiver_registration(&self) -> &PagerPacketReceiverRegistration<Self> {
                &self.pager_packet_receiver_registration
            }

            fn vmo(&self) -> &zx::Vmo {
                &self.vmo
            }

            fn page_in(self: Arc<Self>, range: PageInRange<Self>) {
                range.report_failure(*self.status_code.lock());
            }

            fn mark_dirty(self: Arc<Self>, _range: MarkDirtyRange<Self>) {
                unreachable!();
            }

            fn on_zero_children(self: Arc<Self>) {
                unreachable!();
            }

            fn byte_size(&self) -> u64 {
                unreachable!();
            }

            async fn aligned_read(
                &self,
                _aligned_byte_range: std::ops::Range<u64>,
            ) -> Result<buffer::Buffer<'_>, Error> {
                unreachable!();
            }
        }

        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let file = Arc::new_cyclic(|weak| {
            let (vmo, pager_packet_receiver_registration) =
                pager.create_vmo(weak.clone(), page_size(), zx::VmoOptions::empty()).unwrap();
            StatusCodeFile {
                vmo,
                pager: pager.clone(),
                status_code: Mutex::new(zx::Status::INTERNAL),
                pager_packet_receiver_registration,
            }
        });

        fn check_mapping(
            file: &StatusCodeFile,
            failure_code: zx::Status,
            expected_code: zx::Status,
        ) {
            {
                *file.status_code.lock() = failure_code;
            }
            let mut buf = [0u8; 8];
            assert_eq!(file.vmo().read(&mut buf, 0).unwrap_err(), expected_code);
        }
        check_mapping(&file, zx::Status::IO_DATA_INTEGRITY, zx::Status::IO_DATA_INTEGRITY);
        check_mapping(&file, zx::Status::NO_SPACE, zx::Status::NO_SPACE);
        check_mapping(&file, zx::Status::FILE_BIG, zx::Status::BUFFER_TOO_SMALL);
        check_mapping(&file, zx::Status::IO, zx::Status::IO);
        check_mapping(&file, zx::Status::IO_DATA_LOSS, zx::Status::IO);
        check_mapping(&file, zx::Status::NOT_EMPTY, zx::Status::BAD_STATE);
        check_mapping(&file, zx::Status::BAD_STATE, zx::Status::BAD_STATE);

        scope.wait().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_query_vmo_stats() {
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let file = MockFile::new(pager.clone());

        let stats = pager.query_vmo_stats(file.vmo(), PagerVmoStatsOptions::empty()).unwrap();
        // The VMO hasn't been modified yet.
        assert!(!stats.was_vmo_modified());

        file.vmo().write(&[0, 1, 2, 3, 4], 0).unwrap();
        let stats = pager.query_vmo_stats(file.vmo(), PagerVmoStatsOptions::empty()).unwrap();
        assert!(stats.was_vmo_modified());

        // Reset the stats this time.
        let stats =
            pager.query_vmo_stats(file.vmo(), PagerVmoStatsOptions::RESET_VMO_STATS).unwrap();
        // The stats weren't reset last time so the stats are still showing that the vmo is modified.
        assert!(stats.was_vmo_modified());

        let stats = pager.query_vmo_stats(file.vmo(), PagerVmoStatsOptions::empty()).unwrap();
        assert!(!stats.was_vmo_modified());

        scope.wait().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_query_dirty_ranges() {
        // Some notes on our paging implementation:
        //  * Fxfs uses UNBOUNDED VMO. These are maximally sized at creation time with
        //    stream size holding the content length.
        //  * Like regular VMO, all pages are initially in an unknown state. When a page
        //    is first accessed, the pager (Fxfs) will be asked to page in content.
        //  * Size can be set as a property, via set_content_size or via set_stream_size
        //    but only set_stream_size() should ever be used. This ensures that the tail
        //    is correctly zeroed.
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let file = MockFile::new_with_size_and_type(
            pager.clone(),
            page_size() + page_size() / 2,
            zx::VmoOptions::UNBOUNDED,
        );
        let mut buffer = vec![VmoDirtyRange::default(); 2];

        let page_size = page_size();
        assert_eq!(file.vmo().get_content_size().unwrap(), page_size + page_size / 2);

        let (actual, remaining) =
            pager.query_dirty_ranges(file.vmo(), 0..page_size * 100, &mut buffer).unwrap();
        assert_eq!(actual, 0);
        assert_eq!(remaining, 0);

        // Grow the VMO content size from 1.5 pages to 7.5 pages.
        file.vmo().set_stream_size(page_size * 7 + page_size / 2).unwrap();

        let (actual, remaining) =
            pager.query_dirty_ranges(file.vmo(), 0..page_size * 100, &mut buffer).unwrap();
        assert_eq!(actual, 2);
        assert_eq!(remaining, 0);
        // Second page must be assumed to contain data so tail is zeroed.
        assert_eq!(buffer[0].range(), page_size..page_size * 2);
        assert!(!buffer[0].is_zero_range());
        // All pages after that are marked as zero.
        assert_eq!(buffer[1].range(), page_size * 2..page_size * 8);
        assert!(buffer[1].is_zero_range());

        // We expect the tail page to have been read as part of the zeroing when we grew the size.
        // It will then be marked dirty (modified)
        assert_eq!(
            file.pager_requests(true),
            vec![
                PagerRequest::PageIn(page_size * 1..page_size * 2),
                PagerRequest::Dirty(page_size * 1..page_size * 2),
            ]
        );

        // Modify the 2nd, 3rd, and 5th pages.
        file.vmo().write(&[1, 2, 3, 4], page_size).unwrap();
        file.vmo().write(&[1, 2, 3, 4], page_size * 2).unwrap();
        file.vmo().write(&[1, 2, 3, 4], page_size * 4).unwrap();

        // Pages are known zero because we just grew the file.
        // We don't expect any page-in requests for them.
        assert_eq!(
            file.pager_requests(true),
            vec![
                PagerRequest::Dirty(page_size * 2..page_size * 3),
                PagerRequest::Dirty(page_size * 4..page_size * 5)
            ]
        );

        let (actual, remaining) =
            pager.query_dirty_ranges(file.vmo(), 0..page_size * 7, &mut buffer).unwrap();
        assert_eq!(actual, 2);
        assert_eq!(remaining, 2);
        // Second and third pages (non-zero)
        assert_eq!(buffer[0].range(), page_size..(page_size * 3));
        assert!(!buffer[0].is_zero_range());
        // Fourth page is zero.
        assert_eq!(buffer[1].range(), (page_size * 3)..(page_size * 4));
        assert!(buffer[1].is_zero_range());

        let (actual, remaining) = pager
            .query_dirty_ranges(file.vmo(), page_size * 4..page_size * 7, &mut buffer)
            .unwrap();
        assert_eq!(actual, 2);
        assert_eq!(remaining, 0);
        // Fifth page (non-zero)
        assert_eq!(buffer[0].range(), (page_size * 4)..(page_size * 5));
        assert!(!buffer[0].is_zero_range());
        // Rest of the VMO is zero.
        assert_eq!(buffer[1].range(), (page_size * 5)..(page_size * 7));
        assert!(buffer[1].is_zero_range());

        // Read the 4th page.
        let mut read_buf = vec![0u8; page_size as usize];
        file.vmo().read(&mut read_buf, page_size * 3).expect("read");
        let expected = vec![0u8; page_size as usize];
        assert_eq!(read_buf, expected);
        assert_eq!(file.pager_requests(true), vec![]);

        scope.wait().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_zero_grown_vmo() {
        // When a VMO's content size is explicitly grown, check that new content is zeroed.
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let file = MockFile::new(pager.clone());

        let write_buf = vec![0xff; page_size() as usize * 2];
        file.vmo().set_stream_size(page_size() * 2).expect("grow");
        file.vmo().write(&write_buf, 0).expect("write");
        let mut read_buf = vec![0u8; page_size() as usize * 2];
        // The tail beyond the content size will written.
        file.vmo().read(&mut read_buf, 0).expect("read");
        assert_eq!(read_buf, write_buf);

        // The tail beyond the new content size should be zeroed.
        file.vmo().set_stream_size(page_size() + 1).expect("shrink");
        file.vmo().write(&[0xff; 3], page_size() + 2).expect("write after shrink");
        // To make sure the above content size change actually zeroed data, we grow again.
        file.vmo().set_stream_size(page_size() + 4).expect("grow again");
        let mut read_buf = vec![0u8; page_size() as usize];
        file.vmo().read(&mut read_buf, page_size()).expect("read");
        let mut expected = vec![0u8; page_size() as usize];
        expected[0] = 0xff;
        assert_eq!(read_buf, expected);

        scope.wait().await;
    }

    #[fuchsia::test]
    async fn test_pager_range_chunks_iter_chunks() {
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope).unwrap());
        let file = MockFile::new(pager.clone());

        let pager_range =
            PageInRange::new(0..page_size() * 5, OpenedNode::new(file), Epoch::global().guard());
        let ranges: Vec<Range<u64>> = pager_range
            .chunks(page_size() * 2)
            .map(|pager_range| {
                let range = pager_range.range();
                pager_range.consume();
                range
            })
            .collect();
        assert_eq!(
            ranges,
            [
                0..page_size() * 2,
                page_size() * 2..page_size() * 4,
                page_size() * 4..page_size() * 5
            ]
        );
    }

    #[fuchsia::test]
    async fn test_pager_range_split() {
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope).unwrap());
        let file = MockFile::new(pager.clone());

        let pager_range =
            PageInRange::new(0..page_size() * 10, OpenedNode::new(file), Epoch::global().guard());
        let (left, right) = pager_range.split(page_size() * 5);
        let (left, right) = (left.unwrap(), right.unwrap());
        assert_eq!(left.range(), 0..page_size() * 5);
        assert_eq!(right.range(), page_size() * 5..page_size() * 10);

        left.consume();
        right.consume();
    }

    #[fuchsia::test]
    #[should_panic(expected = "0..8192 is not a subset of 0..4096")]
    async fn test_pager_range_bad_expand_panics() {
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope).unwrap());
        let file = MockFile::new(pager.clone());

        let pager_range =
            PageInRange::new(0..page_size() * 2, OpenedNode::new(file), Epoch::global().guard());
        pager_range.expand(0..page_size()).consume();
    }

    #[derive(ToWeakNode)]
    struct PagerRangeTestFile {
        vmo: zx::Vmo,
        pager_packet_receiver_registration: PagerPacketReceiverRegistration<Self>,
        pager: Pager,
        page_in_fn: Box<dyn Fn(PageInRange<Self>) + Send + Sync + 'static>,
        mark_dirty_fn: Box<dyn Fn(MarkDirtyRange<Self>) + Send + Sync + 'static>,
    }

    impl PagerRangeTestFile {
        fn new<
            F1: Fn(PageInRange<Self>) + Send + Sync + 'static,
            F2: Fn(MarkDirtyRange<Self>) + Send + Sync + 'static,
        >(
            page_in_fn: F1,
            mark_dirty_fn: F2,
        ) -> Arc<Self> {
            Arc::new_cyclic(|weak| {
                let pager = Pager::new(ExecutionScope::new()).unwrap();
                let (vmo, pager_packet_receiver_registration) = pager
                    .create_vmo(weak.clone(), page_size() * 2, zx::VmoOptions::TRAP_DIRTY)
                    .unwrap();
                Self {
                    vmo,
                    pager_packet_receiver_registration,
                    pager,
                    page_in_fn: Box::new(page_in_fn),
                    mark_dirty_fn: Box::new(mark_dirty_fn),
                }
            })
        }
    }

    impl FxNode for PagerRangeTestFile {
        fn object_id(&self) -> u64 {
            1
        }

        fn parent(&self) -> Option<Arc<crate::directory::FxDirectory>> {
            unimplemented!()
        }

        fn set_parent(&self, _parent: Arc<crate::directory::FxDirectory>) {
            unimplemented!()
        }

        fn open_count_add_one(&self) {}

        fn open_count_sub_one(self: Arc<Self>) {}

        fn object_descriptor(&self) -> fxfs::object_store::ObjectDescriptor {
            unimplemented!()
        }
    }

    impl PagerBacked for PagerRangeTestFile {
        fn try_keep_open(self: Arc<Self>) -> Result<OpenedNode<Self>, Arc<Self>> {
            Ok(OpenedNode(self))
        }

        fn pager(&self) -> &Pager {
            &self.pager
        }

        fn pager_packet_receiver_registration(&self) -> &PagerPacketReceiverRegistration<Self> {
            &self.pager_packet_receiver_registration
        }

        fn vmo(&self) -> &zx::Vmo {
            &self.vmo
        }

        fn page_in(self: Arc<Self>, range: PageInRange<Self>) {
            (self.page_in_fn)(range)
        }

        fn mark_dirty(self: Arc<Self>, range: MarkDirtyRange<Self>) {
            (self.mark_dirty_fn)(range)
        }

        fn on_zero_children(self: Arc<Self>) {}

        fn byte_size(&self) -> u64 {
            unimplemented!();
        }

        async fn aligned_read(
            &self,
            _range: std::ops::Range<u64>,
        ) -> Result<buffer::Buffer<'_>, Error> {
            unimplemented!();
        }
    }

    fn real_supply_pages(range: PageInRange<PagerRangeTestFile>) {
        let aux_vmo = zx::Vmo::create(range.len()).unwrap();
        range.supply_pages(&aux_vmo, 0);
    }

    fn real_mark_dirty(range: MarkDirtyRange<PagerRangeTestFile>) {
        let _ = range.dirty_pages();
    }

    #[fuchsia::test(threads = 2)]
    async fn test_page_in_range_supply_pages() {
        let file = PagerRangeTestFile::new(real_supply_pages, real_mark_dirty);

        let mut data = vec![0; 20];
        file.vmo.read(&mut data, 0).unwrap();
    }

    #[fuchsia::test(threads = 2)]
    async fn test_page_in_range_report_failure() {
        let file = PagerRangeTestFile::new(
            |range| {
                range.report_failure(zx::Status::IO_DATA_INTEGRITY);
            },
            real_mark_dirty,
        );

        let mut data = vec![0; 20];
        let err = file.vmo.read(&mut data, 0).unwrap_err();
        assert_eq!(err, zx::Status::IO_DATA_INTEGRITY);
    }

    #[cfg(debug_assertions)]
    #[fuchsia::test(threads = 2)]
    #[should_panic(expected = "PagerRange was dropped without sending a response")]
    async fn test_page_in_range_dropped() {
        let file = PagerRangeTestFile::new(|_| {}, real_mark_dirty);

        let mut data = vec![0; 20];
        file.vmo.read(&mut data, 0).unwrap_err();
    }

    #[cfg(not(debug_assertions))]
    #[fuchsia::test(threads = 2)]
    async fn test_page_in_range_dropped() {
        let file = PagerRangeTestFile::new(|_| {}, real_mark_dirty);

        let mut data = vec![0; 20];
        let err = file.vmo.read(&mut data, 0).unwrap_err();
        assert_eq!(err, zx::Status::BAD_STATE);
    }

    #[fuchsia::test(threads = 2)]
    async fn test_mark_dirty_range_dirty_pages() {
        let file = PagerRangeTestFile::new(real_supply_pages, real_mark_dirty);

        let data = vec![5; 20];
        file.vmo.write(&data, 0).unwrap();
    }

    #[fuchsia::test(threads = 2)]
    async fn test_mark_dirty_range_report_failure() {
        let file = PagerRangeTestFile::new(real_supply_pages, |range| {
            range.report_failure(zx::Status::IO_DATA_INTEGRITY);
        });

        let data = vec![5; 20];
        let err = file.vmo.write(&data, 0).unwrap_err();
        assert_eq!(err, zx::Status::IO_DATA_INTEGRITY);
    }

    #[cfg(debug_assertions)]
    #[fuchsia::test(threads = 2)]
    #[should_panic(expected = "PagerRange was dropped without sending a response")]
    async fn test_mark_dirty_range_dropped() {
        let file = PagerRangeTestFile::new(real_supply_pages, |_| {});

        let data = vec![5; 20];
        file.vmo.write(&data, 0).unwrap_err();
    }

    #[cfg(not(debug_assertions))]
    #[fuchsia::test(threads = 2)]
    async fn test_mark_dirty_range_dropped() {
        let file = PagerRangeTestFile::new(real_supply_pages, |_| {});

        let data = vec![5; 20];
        let err = file.vmo.write(&data, 0).unwrap_err();
        assert_eq!(err, zx::Status::BAD_STATE);
    }

    #[fuchsia::test(threads = 2)]
    async fn test_pager_range_chunks_iter_consumed() {
        let file = PagerRangeTestFile::new(
            |range| {
                let aux_vmo = zx::Vmo::create(page_size()).unwrap();
                range.expand(0..page_size() * 2).chunks(page_size()).for_each(|range| {
                    range.supply_pages(&aux_vmo, 0);
                });
            },
            real_mark_dirty,
        );

        let mut data = vec![0; 20];
        file.vmo.read(&mut data, 0).unwrap();
    }

    fn partial_supply_pages(range: PageInRange<PagerRangeTestFile>) {
        let aux_vmo = zx::Vmo::create(page_size()).unwrap();
        // Expand the range to 2 pages and only supply the first page, dropping the iterator without
        // fully consuming it.
        range.expand(0..page_size() * 2).chunks(page_size()).take(1).for_each(|range| {
            range.supply_pages(&aux_vmo, 0);
        });
    }

    #[cfg(debug_assertions)]
    #[fuchsia::test(threads = 2)]
    #[should_panic(expected = "PagerRangeChunksIter was dropped without being fully consumed")]
    async fn test_pager_range_chunks_iter_dropped() {
        let file = PagerRangeTestFile::new(partial_supply_pages, real_mark_dirty);

        let mut data = vec![0; 20];
        // Ask for the 2nd page. The range will be expanded to the first 2 pages. The first page
        // will succeed and the second page will be dropped.
        file.vmo.read(&mut data, page_size()).unwrap_err();
    }

    #[cfg(not(debug_assertions))]
    #[fuchsia::test(threads = 2)]
    async fn test_pager_range_chunks_iter_dropped() {
        let file = PagerRangeTestFile::new(partial_supply_pages, real_mark_dirty);

        let mut data = vec![0; 20];
        // Ask for the 2nd page. The range will be expanded to the first 2 pages. The first page
        // will succeed and the second page will be dropped.
        let err = file.vmo.read(&mut data, page_size()).unwrap_err();
        assert_eq!(err, zx::Status::BAD_STATE);
    }

    #[fuchsia::test(threads = 2)]
    async fn test_grow_zeroes_new_bytes() {
        // We expect that when we grow a file, the pages between the old and the new size
        // are zeroed. Reads and writes to these pages after growing a file should NOT
        // trigger any page-in requests.
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let page_size = page_size();
        let vmo_size: u64 = page_size * 2;
        let file_a =
            MockFile::new_with_size_and_type(pager.clone(), vmo_size, zx::VmoOptions::RESIZABLE);
        let file_b =
            MockFile::new_with_size_and_type(pager.clone(), vmo_size, zx::VmoOptions::UNBOUNDED);
        let mut buffer = vec![VmoDirtyRange::default(); 3];

        assert_eq!(file_a.vmo().get_stream_size().unwrap(), page_size * 2);
        assert_eq!(file_b.vmo().get_stream_size().unwrap(), page_size * 2);

        // Page in is expected.
        let mut read_buf = vec![0u8; page_size as usize];
        file_a.vmo().read(&mut read_buf, page_size).expect("read a");
        assert_eq!(
            file_a.pager_requests(true),
            vec![PagerRequest::PageIn(page_size..page_size * 2)]
        );
        file_b.vmo().read(&mut read_buf, page_size).expect("read b");
        assert_eq!(
            file_b.pager_requests(true),
            vec![PagerRequest::PageIn(page_size..page_size * 2)]
        );

        // Grow the VMO size and confirm intermediate pages (2..8) are zero.
        let vmo_size = page_size * 8;
        file_a.vmo().set_size(vmo_size).unwrap();
        file_b.vmo().set_stream_size(vmo_size).unwrap();

        assert_eq!(
            pager.query_dirty_ranges(file_a.vmo(), 0..vmo_size, &mut buffer).unwrap(),
            (1, 0)
        );
        assert_eq!(
            buffer[0],
            VmoDirtyRange { offset: page_size * 2, length: page_size * 6, options: 1 },
        );
        assert_eq!(
            pager.query_dirty_ranges(file_b.vmo(), 0..vmo_size, &mut buffer).unwrap(),
            (1, 0)
        );
        assert_eq!(
            buffer[0],
            VmoDirtyRange { offset: page_size * 2, length: page_size * 6, options: 1 },
        );

        // The extra pages are all zero. We shouldn't see any page_in requests.
        let mut read_buf = vec![0u8; page_size as usize * 6];
        file_a.vmo().read(&mut read_buf, page_size * 2).expect("read a");
        assert_eq!(file_a.pager_requests(true), vec![]);
        file_b.vmo().read(&mut read_buf, page_size * 2).expect("read b");
        assert_eq!(file_b.pager_requests(true), vec![]);

        // Grow again and check that pager gets notified.
        let vmo_size = page_size * 8;
        file_a.vmo().set_size(vmo_size).unwrap();
        file_b.vmo().set_stream_size(vmo_size).unwrap();
        assert_eq!(
            pager.query_dirty_ranges(file_a.vmo(), 0..vmo_size, &mut buffer).unwrap(),
            (1, 0)
        );
        assert_eq!(
            buffer[0],
            VmoDirtyRange { offset: page_size * 2, length: page_size * 6, options: 1 },
        );
        assert_eq!(
            pager.query_dirty_ranges(file_b.vmo(), 0..vmo_size, &mut buffer).unwrap(),
            (1, 0)
        );
        assert_eq!(
            buffer[0],
            VmoDirtyRange { offset: page_size * 2, length: page_size * 6, options: 1 },
        );
        // No pager requests. All new pages are assumed zero.
        assert_eq!(file_a.pager_requests(true), vec![],);
        assert_eq!(file_b.pager_requests(true), vec![],);

        // Modifying a page in this new region should trigger a dirty message to the pager.
        file_b.vmo().write(&[1; 10], page_size * 2).unwrap();
        assert_eq!(
            file_b.pager_requests(true),
            vec![PagerRequest::Dirty(page_size * 2..page_size * 3)],
        );

        // Shrink again to 4 pages and then append a page via zx_stream_write (WRITE)
        let vmo_size = page_size * 4;
        file_b.vmo().set_stream_size(vmo_size).unwrap();
        let stream =
            zx::Stream::create(zx::StreamOptions::MODE_WRITE, file_b.vmo(), page_size * 4).unwrap();
        stream.write(zx::StreamWriteOptions::empty(), &vec![10; page_size as usize]).unwrap();
        assert_eq!(
            file_b.pager_requests(true),
            vec![PagerRequest::Dirty(page_size * 4..page_size * 5)],
        );

        // Append a page via zx_stream_write (APPEND)
        let stream = zx::Stream::create(
            zx::StreamOptions::MODE_WRITE | zx::StreamOptions::MODE_APPEND,
            file_b.vmo(),
            page_size * 5,
        )
        .unwrap();
        stream.write(zx::StreamWriteOptions::empty(), &[10; 1024]).unwrap();
        assert_eq!(
            file_b.pager_requests(true),
            vec![PagerRequest::Dirty(page_size * 5..page_size * 6)],
        );

        scope.wait().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_pathological_shrink_unbounded_vmo() {
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let page_size = page_size();
        let vmo_size: u64 = page_size * 25600; // 100MiB
        let file =
            MockFile::new_with_size_and_type(pager.clone(), vmo_size, zx::VmoOptions::UNBOUNDED);
        let mut buffer = vec![VmoDirtyRange::default(); 10];

        assert_eq!(file.vmo().get_stream_size().unwrap(), vmo_size);

        // Shrinking by a small step to check that last page truncation works as expected.
        for i in 0..vmo_size / 256 {
            let data = vec![5; 20];
            file.vmo.write(&data, i * 256).expect("write failed");
        }

        for i in (0..25600u64 / 1024).rev() {
            file.vmo().set_stream_size(i * 1024 + page_size / 2).unwrap();
        }

        assert_eq!(pager.query_dirty_ranges(file.vmo(), 0..vmo_size, &mut buffer).unwrap(), (1, 0));
        assert_eq!(buffer[0..1], [VmoDirtyRange { offset: 0, length: page_size, options: 0 },]);

        scope.wait().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_pathological_shrink_unbounded_vmo_with_gaps() {
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let page_size = page_size();
        let vmo_size: u64 = page_size * 25600; // 100MiB
        let file =
            MockFile::new_with_size_and_type(pager.clone(), vmo_size, zx::VmoOptions::UNBOUNDED);
        let mut buffer = vec![VmoDirtyRange::default(); 10];

        assert_eq!(file.vmo().get_stream_size().unwrap(), vmo_size);

        // Write every second page.
        for offset in (0u64..vmo_size).step_by((page_size * 2) as usize) {
            let data = vec![5; 20];
            file.vmo.write(&data, offset).expect("write failed");
        }
        // Every second page should be dirty.
        let (actual, remaining) =
            pager.query_dirty_ranges(file.vmo(), 0..vmo_size, &mut buffer).unwrap();
        assert_eq!(actual + remaining, 25600 / 2);

        // Avoid page-aligned sizes to ensure we test the partial page code paths.
        let mut offset = vmo_size.saturating_sub(5 * page_size - 2);
        // Shrink by 5 pages, then 4 pages. This covers all possible arrangements of
        // start/end being on zero and non-zero pages.
        'outer: loop {
            for delta in [5 * page_size, 4 * page_size] {
                file.vmo().set_stream_size(offset).unwrap();
                assert_eq!(
                    pager.query_dirty_ranges(file.vmo(), offset..vmo_size, &mut buffer).unwrap(),
                    (1, 0)
                );
                // We do not expect to see dirty pages beyond stream size.
                assert_eq!(
                    buffer[0..1],
                    [VmoDirtyRange {
                        offset: round_down(offset, page_size),
                        length: page_size,
                        options: 0
                    },]
                );
                offset = offset.saturating_sub(delta);
                if offset == 0 {
                    break 'outer;
                }
            }
        }

        scope.wait().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn test_grow_unbounded_vmo() {
        let scope = ExecutionScope::new();
        let pager = Arc::new(Pager::new(scope.clone()).unwrap());
        let file = MockFile::new_with_size_and_type(pager.clone(), 128, zx::VmoOptions::UNBOUNDED);

        let data = vec![1; 128];
        // Overwrite the 128 after the content size;
        file.vmo().write(&data, 128).expect("write failed");
        // Grow the VMO to include the newly written bytes.
        file.vmo().set_stream_size(256).unwrap();
        assert_eq!(file.vmo().get_stream_size().expect("get_stream_size"), 256);

        let mut data = vec![0xff; 256];
        file.vmo().read(&mut data, 0).expect("read");
        let expected = vec![0; 256];
        assert_eq!(data, expected);

        file.vmo().set_stream_size(page_size() * 3).unwrap();
        let mut buffer = vec![VmoDirtyRange::default(); 10];
        assert_eq!(
            pager.query_dirty_ranges(file.vmo(), 0..page_size() * 3, &mut buffer).unwrap(),
            (2, 0)
        );
        // We expect to see only zero pages beyond content size.
        assert_eq!(
            buffer[0..2],
            [
                VmoDirtyRange { offset: 0, length: page_size(), options: 0 },
                VmoDirtyRange { offset: page_size(), length: page_size() * 2, options: 1 },
            ]
        );

        scope.wait().await;
    }
}
