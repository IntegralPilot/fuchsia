// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::file::FlushType;
use crate::fuchsia::pager::{
    MarkDirtyRange, Pager, PagerBacked, PagerVmoStatsOptions, VmoDirtyRange,
};
use crate::fuchsia::volume::FxVolume;
use anyhow::{Context, Error, anyhow, bail, ensure};
use fidl_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use fxfs::errors::FxfsError;
use fxfs::filesystem::{MAX_FILE_SIZE, TruncateGuard};
use fxfs::log::*;
use fxfs::object_handle::{ObjectHandle, ObjectProperties, ReadObjectHandle};
use fxfs::object_store::allocator::{Allocator, Reservation, ReservationOwner};
use fxfs::object_store::transaction::{
    LockKey, Options, TRANSACTION_METADATA_MAX_AMOUNT, Transaction, lock_keys,
};
use fxfs::object_store::{
    AttributeId, DataObjectHandle, ObjectStore, RangeType, StoreObjectHandle, Timestamp,
};
use fxfs::range::RangeExt;
use scopeguard::defer;
use std::future::Future;
use std::ops::Range;
use std::sync::Arc;
use storage_device::buffer::{Buffer, BufferFuture};
use storage_units::{BlockSize, PAGE_SIZE};
use vfs::temp_clone::{TempClonable, unblock};

/// How much data each sync transaction in a given flush will cover.
pub const FLUSH_BATCH_SIZE: u64 = 524_288;
/// The amount of dirty bytes to trigger a background flush. This needs to be at least double the
/// `FLUSH_BATCH_SIZE` because bytes can be reserved and unreserved which are split into separate
/// batches and this value needs to ensure that at least one of the two dirty page states has enough
/// for a full batch. Just using the minimum is a bit aggressive though, going higher.
pub const BACKGROUND_FLUSH_THRESHOLD: u64 = FLUSH_BATCH_SIZE * 20;

/// An expanding write will: mark a page as dirty, write to the page, and then update the content
/// size. If a flush is triggered during an expanding write then query_dirty_ranges may return pages
/// that have been marked dirty but are beyond the stream size. Those extra pages can't be cleaned
/// during the flush and will have to be cleaned in a later flush. The initial flush will consume
/// the transaction metadata space that the extra pages were supposed to be part of leaving no
/// transaction metadata space for the extra pages in the next flush if no additional pages are
/// dirtied. `SPARE_SIZE` is extra metadata space that gets reserved be able to flush the extra
/// pages if this situation occurs.
const SPARE_SIZE: u64 = TRANSACTION_METADATA_MAX_AMOUNT;

// Since marking pages dirty takes the `inner` lock, and `inner.take()` for flushing must also, the
// call to `inner.take()` is like an interaction with the kernel, and can be considered an atomic
// query of the number of dirty page requests received so far. The collecting of ranges can also be
// considered an atomic interaction with the kernel, even though it is a series of calls, because of
// how the request never actually queries about the same range twice and it doesn't matter if they
// are received out of order. We call `inner.take()` before collecting ranges and then again if the
// values in the collection don't match. This makes for 3 atomic steps where interactions can race
// change the kernel state in between them. These two callbacks allow us to place actions between
// the atomic steps in order to coerce whatever ordering we want.
#[cfg(test)]
use crate::fuchsia::testing::TestCallback;
#[cfg(test)]
static CALLBACK_BEFORE_RANGE_COLLECTION: TestCallback = TestCallback::new();
#[cfg(test)]
static CALLBACK_AFTER_RANGE_COLLECTION: TestCallback = TestCallback::new();
#[cfg(test)]
static CALLBACK_AFTER_SIZE_CAPTURE: TestCallback = TestCallback::new();
#[cfg(test)]
static CALLBACK_AFTER_READ_UNCACHED: TestCallback = TestCallback::new();

pub struct PagedObjectHandle {
    inner: Mutex<Inner>,
    vmo: TempClonable<zx::Vmo>,
    handle: DataObjectHandle<FxVolume>,
}

#[derive(Clone, Copy, Debug, Default)]
struct DirtyPages {
    /// Pages that need a reservation in the allocator to write them back and mark them clean.
    reserved: u64,

    /// Pages that do not need a reservation in the allocator to mark them clean.
    unreserved: u64,
}

impl DirtyPages {
    fn total(&self) -> u64 {
        self.reserved + self.unreserved
    }
}

impl std::ops::AddAssign for DirtyPages {
    fn add_assign(&mut self, rhs: Self) {
        self.unreserved += rhs.unreserved;
        self.reserved += rhs.reserved;
    }
}

impl std::ops::Sub for DirtyPages {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            reserved: self.reserved - rhs.reserved,
            unreserved: self.unreserved - rhs.unreserved,
        }
    }
}

impl std::ops::SubAssign for DirtyPages {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl std::ops::Add for DirtyPages {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            reserved: self.reserved + rhs.reserved,
            unreserved: self.unreserved + rhs.unreserved,
        }
    }
}

#[derive(Debug)]
struct Inner {
    dirty_crtime: DirtyTimestamp,
    dirty_mtime: DirtyTimestamp,

    /// The number of pages that have been marked dirty by the kernel and need to be cleaned.
    dirty_pages: DirtyPages,

    /// The amount of extra space currently reserved. See `SPARE_SIZE`.
    spare: u64,

    /// Stores whether the file needs to be shrunk or trimmed during the next flush.
    pending_shrink: PendingShrink,

    /// This bit is set at the top of enable_verity(). Once this bit is set, all future calls to
    /// mark_dirty() and truncate() will fail. This ensures that the contents and size of the
    /// file do not change while the merkle tree is being computed or thereon after.
    read_only: bool,

    /// True if the file is currently being flushed. There can be only one task flushing at a time.
    flushing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PendingShrink {
    None,

    /// The file needs to be shrunk during the next flush. After shrinking the file, the file may
    /// then also need to be trimmed. We also stash whether or not we need to update the
    /// has_overwrite_extents metadata flag during the shrink, because we get rid of the in-memory
    /// tracking of the overwrite extents immediately but we can't update the on-disk metadata
    /// until the next flush.
    ShrinkTo(u64, Option<bool>),

    /// The file needs to be trimmed during the next flush.
    NeedsTrim,
}

// DirtyTimestamp tracks a dirty timestamp and handles flushing. Whilst we're flushing, we need to
// hang on to the timestamp in case anything queries it, but once we've finished, we can discard it
// so long as it hasn't been written again.
#[derive(Clone, Copy, Debug)]
enum DirtyTimestamp {
    None,
    Some(Timestamp),
    PendingFlush(Timestamp),
}

impl DirtyTimestamp {
    // If we have a timestamp, move to the PendingFlush state.
    fn begin_flush(&mut self, update_to_now: bool) -> Option<Timestamp> {
        if update_to_now {
            let now = Timestamp::now();
            *self = DirtyTimestamp::PendingFlush(now);
            Some(now)
        } else {
            match self {
                DirtyTimestamp::None => None,
                DirtyTimestamp::Some(t) => {
                    let t = *t;
                    *self = DirtyTimestamp::PendingFlush(t);
                    Some(t)
                }
                DirtyTimestamp::PendingFlush(t) => Some(*t),
            }
        }
    }

    // We finished a flush, so discard it if no further update was made.
    fn end_flush(&mut self) {
        if let DirtyTimestamp::PendingFlush(_) = self {
            *self = DirtyTimestamp::None;
        }
    }

    fn timestamp(&self) -> Option<Timestamp> {
        match self {
            DirtyTimestamp::None => None,
            DirtyTimestamp::Some(t) => Some(*t),
            DirtyTimestamp::PendingFlush(t) => Some(*t),
        }
    }

    fn needs_flush(&self) -> bool {
        !matches!(self, DirtyTimestamp::None)
    }
}

impl std::convert::From<Option<Timestamp>> for DirtyTimestamp {
    fn from(value: Option<Timestamp>) -> Self {
        if let Some(t) = value { DirtyTimestamp::Some(t) } else { DirtyTimestamp::None }
    }
}

/// Returns the amount of space that should be reserved to be able to flush `page_count` pages.
fn reservation_needed(page_count: u64) -> u64 {
    let pages_per_transaction = FLUSH_BATCH_SIZE / PAGE_SIZE;
    let transaction_count = page_count.div_ceil(pages_per_transaction);
    transaction_count * TRANSACTION_METADATA_MAX_AMOUNT + page_count * PAGE_SIZE
}

/// Returns the number of pages spanned by `range`. `range` must be page aligned.
fn page_count(range: Range<u64>) -> u64 {
    debug_assert!(range.start <= range.end);
    debug_assert!(PAGE_SIZE.is_aligned(&range), "range not page aligned: {range:?}");
    (range.end - range.start) / PAGE_SIZE
}

impl Inner {
    fn new(read_only: bool) -> Mutex<Self> {
        Mutex::new(Self {
            dirty_crtime: DirtyTimestamp::None,
            dirty_mtime: DirtyTimestamp::None,
            dirty_pages: DirtyPages { reserved: 0, unreserved: 0 },
            spare: 0,
            pending_shrink: PendingShrink::None,
            read_only,
            flushing: false,
        })
    }

    fn reservation(&self) -> u64 {
        reservation_needed(self.dirty_pages.reserved) + self.spare
    }

    /// Takes all the dirty pages with reservations and returns (<DirtyPages>, <Reservation>).
    fn take(
        &mut self,
        allocator: Arc<Allocator>,
        store_object_id: u64,
    ) -> (DirtyPages, Reservation) {
        let reservation = allocator.reserve_with(Some(store_object_id), |_| 0);
        reservation.add(self.reservation());
        self.spare = 0;
        (std::mem::take(&mut self.dirty_pages), reservation)
    }

    /// Takes all the dirty pages and adds to the reservation.
    fn move_to(&mut self, reservation: &Reservation) -> DirtyPages {
        reservation.add(self.reservation());
        self.spare = 0;
        std::mem::take(&mut self.dirty_pages)
    }

    /// Put back some dirty pages taking from reservation as required.
    fn put_back(&mut self, dirty_pages: DirtyPages, reservation: &Reservation) {
        if dirty_pages.reserved > 0 {
            let before = self.reservation();
            self.dirty_pages += dirty_pages;
            let needed = reservation_needed(self.dirty_pages.reserved);
            self.spare = std::cmp::min(reservation.amount() + before - needed, SPARE_SIZE);
            reservation.forget_some(needed + self.spare - before);
        } else {
            self.dirty_pages += dirty_pages;
        }
    }

    /// Return the reservation to the allocator, and return the number of currently dirty pages.
    fn forget_dirty_pages(
        &mut self,
        allocator: Arc<Allocator>,
        store_object_id: u64,
    ) -> DirtyPages {
        allocator.release_reservation(Some(store_object_id), self.reservation());
        self.spare = 0;
        std::mem::take(&mut self.dirty_pages)
    }

    fn end_flush(&mut self) {
        self.dirty_mtime.end_flush();
        self.dirty_crtime.end_flush();
    }

    fn needs_flush(&self) -> bool {
        // There should be no need to call `was_file_modified_since_last_call` because we check
        // `dirty_pages` here and there shouldn't be anything to flush if `dirty_pages` is zero
        // and `mtime` does not need flushing (truncating a file can leave `dirty_pages` as
        // zero but it will always update `mtime`).
        self.dirty_crtime.needs_flush()
            || self.dirty_mtime.needs_flush()
            || self.dirty_pages.total() > 0
            || self.pending_shrink != PendingShrink::None
            || self.flushing
    }
}

struct FlushState {
    /// Allocator reservation for space on disk.
    reservation: Reservation,

    /// The number of pages recorded through `MarkDirty` calls that this flush knows about.
    marked_dirty_pages: DirtyPages,

    /// The number of pages we expect to flush, from the collected flush batches.
    pages_to_flush: DirtyPages,

    /// The number of pages we have actually flushed.
    pages_flushed: DirtyPages,

    /// The number of dirty pages we won't be flushing either because they're beyond the current end
    /// of the file or we're not flushing all the pages. The reserved pages need to save their
    /// reservation.
    dirty_pages_not_to_flush: DirtyPages,

    /// The type of the flush.
    flush_type: FlushType,

    /// The stage of this flush.
    flush_step: FlushStep,

    /// True when extra pages were taken because of races where after capturing the initial
    /// reservation and dirty page counts more pages got marked dirty while collecting the batches
    /// from the kernel.
    has_extra_reserved: bool,
}

/// The current step that the flush state has been progressed to.
#[derive(Debug, PartialEq)]
enum FlushStep {
    /// The state has a reservation and taken the dirty pages.
    HasDirtyPageReservation,

    /// The state knows how many pages were found dirty, so it knows how many to flush, and not to
    /// flush, but it does not have enough dirty pages to proceed.
    CollectedBatchesShortDirtyPages,

    /// The state knows how many pages were found dirty, so it knows how many to flush, and not to
    /// flush. It is ready to proceed flushing pages, but it may not have successfully flushed any.
    ReadyToFlush,
}

impl FlushState {
    fn new(
        reservation: Reservation,
        marked_dirty_pages: DirtyPages,
        flush_type: FlushType,
    ) -> Self {
        Self {
            reservation,
            marked_dirty_pages,
            pages_to_flush: Default::default(),
            pages_flushed: Default::default(),
            dirty_pages_not_to_flush: Default::default(),
            flush_type,
            flush_step: FlushStep::HasDirtyPageReservation,
            has_extra_reserved: false,
        }
    }

    fn reservation(&self) -> &Reservation {
        &self.reservation
    }

    /// Set the number of dirty pages that expect to be cleaned by the current flush batches,
    /// and the number of reserved pages found past the end of the file. Returns Err if there are
    /// insufficient marked dirty pages or reservation for the flush batches. The use of Result here
    /// is to enforce result checking and error handling.
    fn set_flush_batch_count(
        &mut self,
        dirty_pages_to_flush: DirtyPages,
        dirty_pages_not_to_flush: DirtyPages,
    ) -> Result<(), ()> {
        assert_eq!(self.pages_to_flush.total(), 0, "This should only be called once.");
        assert_eq!(self.flush_step, FlushStep::HasDirtyPageReservation);
        self.pages_to_flush = dirty_pages_to_flush;
        self.dirty_pages_not_to_flush = dirty_pages_not_to_flush;

        // Need to ensure that we are flushing not only the correct number of total pages, but also
        // the correct number of reserved pages to ensure that we have enough reservation. The
        // reservation should also cover the reserved pages not to flush.
        if self.pages_to_flush.reserved + self.dirty_pages_not_to_flush.reserved
            > self.marked_dirty_pages.reserved
            || self.pages_to_flush.unreserved + self.dirty_pages_not_to_flush.unreserved
                > self.marked_dirty_pages.unreserved
        {
            self.flush_step = FlushStep::CollectedBatchesShortDirtyPages;
            Err(())
        } else {
            self.flush_step = FlushStep::ReadyToFlush;
            Ok(())
        }
    }

    /// Take the reservation and dirty pages again. This is called if `set_flush_batch_count()`
    /// takes too many pages for the `marked_dirty_pages` count to handle. It may take too much,
    /// but such will be put back before the end of the flush. This asserts if the total dirty pages
    /// don't meet or exceed the required amount.
    fn take_extra_dirty_pages(&mut self, inner: &mut Inner) {
        assert_eq!(self.flush_step, FlushStep::CollectedBatchesShortDirtyPages);
        self.flush_step = FlushStep::ReadyToFlush;
        self.has_extra_reserved = true;
        self.marked_dirty_pages += inner.move_to(&self.reservation);

        assert!(
            self.reservation.amount() >= reservation_needed(self.pages_to_flush.reserved)
                && self.marked_dirty_pages.total()
                    >= self.pages_to_flush.total() + self.dirty_pages_not_to_flush.total(),
            "reservation: {}, needed: {}, dirty_pages: {:?}, pages_to_flush: {:?}",
            self.reservation.amount(),
            reservation_needed(self.pages_to_flush.reserved),
            self.marked_dirty_pages,
            self.pages_to_flush,
        );
    }

    /// Add batches of pages that have been flushed.
    fn did_flush_pages(&mut self, flushed_pages: DirtyPages) {
        assert_eq!(self.flush_step, FlushStep::ReadyToFlush);
        self.pages_flushed += flushed_pages;
    }

    /// Puts back the dirty pages and reservations that were not used as part of this flush and
    /// returns the total number of pages to mark clean as a result of the batch. The total includes
    /// both pages that were properly cleaned as well as dirty pages that are in excess and should
    /// be marked clean as they no longer exist, likely due to truncation.
    fn finish(self, inner: &Mutex<Inner>) -> u64 {
        assert!(
            self.pages_to_flush.total() >= self.pages_flushed.total(),
            "Should not clean more than it planned to clean."
        );
        if self.flush_step != FlushStep::ReadyToFlush {
            // If we didn't get this far then we don't have the complete state of the world and we
            // haven't begun flushing anything. Put everything back where we found it, and report no
            // progress.
            if self.marked_dirty_pages.total() > 0 {
                let mut inner = inner.lock();
                inner.put_back(self.marked_dirty_pages, &self.reservation);
            }

            return 0;
        }

        let new_dirty_pages = if self.flush_type == FlushType::LastChance {
            // No more pages can be flushed now. Data loss is possible, but returning reservations.
            DirtyPages::default()
        } else if self.pages_flushed.reserved == self.pages_to_flush.reserved
            && self.pages_flushed.unreserved == self.pages_to_flush.unreserved
            && !self.has_extra_reserved
        {
            // In this (common) case, there was no race and we succeeded in flushing
            // all the pages we expected to, so the number of pages we keep are the ones we
            // elected not to flush.
            self.dirty_pages_not_to_flush
        } else if self.pages_flushed.unreserved > self.marked_dirty_pages.unreserved {
            // It's possible that pages can be marked dirty as COW but then get allocated before
            // collecting the ranges, creating a mismatch. This allows for that shift to happen
            // without underflow.
            DirtyPages {
                reserved: self.marked_dirty_pages.total()
                    - self.pages_flushed.total()
                    - self.dirty_pages_not_to_flush.unreserved,
                unreserved: self.dirty_pages_not_to_flush.unreserved,
            }
        } else {
            // In this path, just subtract whatever we successfully flushed. With races or failed
            // writes there is no way of knowing how many dirty pages should or should not be there.
            self.marked_dirty_pages - self.pages_flushed
        };

        if new_dirty_pages.total() > 0 {
            let mut inner = inner.lock();
            inner.put_back(new_dirty_pages, &self.reservation);
        }

        // Report the delta
        self.marked_dirty_pages.total() - new_dirty_pages.total()
    }
}

impl PagedObjectHandle {
    pub fn new(handle: DataObjectHandle<FxVolume>, vmo: zx::Vmo) -> Self {
        let verified_file = handle.is_verified_file();
        Self { vmo: TempClonable::new(vmo), handle, inner: Inner::new(verified_file) }
    }

    pub fn owner(&self) -> &Arc<FxVolume> {
        self.handle.owner()
    }

    pub fn store(&self) -> &ObjectStore {
        self.handle.store()
    }

    pub fn vmo(&self) -> &zx::Vmo {
        &self.vmo
    }

    pub fn pager(&self) -> &Pager {
        self.owner().pager()
    }

    pub fn set_read_only(&self) {
        self.inner.lock().read_only = true
    }

    pub fn is_read_only(&self) -> bool {
        self.inner.lock().read_only
    }

    pub fn get_size(&self) -> u64 {
        self.vmo.get_stream_size().unwrap()
    }

    // If there are keys to fetch, a future is returned that will prefetch them into the cache.
    // The caller must ensure that the object exists until this future is complete.
    pub fn pre_fetch_keys(&self) -> Option<impl Future<Output = ()> + use<>> {
        self.handle.pre_fetch_keys()
    }

    async fn new_transaction<'a>(
        &self,
        reservation: Option<&'a Reservation>,
    ) -> Result<Transaction<'a>, Error> {
        self.store()
            .new_transaction(
                lock_keys![LockKey::object(
                    self.handle.store().store_object_id(),
                    self.handle.object_id()
                )],
                Options {
                    skip_journal_checks: false,
                    borrow_metadata_space: reservation.is_none(),
                    allocator_reservation: reservation,
                    ..Default::default()
                },
            )
            .await
    }

    fn allocator(&self) -> Arc<Allocator> {
        self.store().filesystem().allocator()
    }

    pub fn uncached_handle(&self) -> &DataObjectHandle<FxVolume> {
        &self.handle
    }

    pub fn uncached_size(&self) -> u64 {
        self.handle.get_size()
    }

    pub fn store_handle(&self) -> &StoreObjectHandle<FxVolume> {
        &*self.handle
    }

    pub async fn read_uncached(&self, range: std::ops::Range<u64>) -> Result<Buffer<'_>, Error> {
        let mut buffer = self.handle.allocate_buffer((range.end - range.start) as usize).await;
        let read = self.handle.read(range.start, buffer.as_mut()).await?;
        buffer.subslice_mut(read..).fill(0);

        #[cfg(test)]
        CALLBACK_AFTER_READ_UNCACHED.call();

        Ok(buffer)
    }

    /// Reduce memory footprint of this file if there are outstanding dirty pages.
    pub async fn minimize_memory(&self) -> Result<(), Error> {
        // This is a best-effort call. It is allowed to race with things, and if all the outstanding
        // cached data is metadata we won't save memory anyways, so don't bother. Only looking for
        // dirty pages, or if there are overwrite pages we'll have to take the slow path since those
        // dirty pages don't record any information internally.
        if self.handle.overwrite_ranges_is_empty() && self.inner.lock().reservation() == 0 {
            return Ok(());
        }
        self.flush(FlushType::Sync).await
    }

    /// Attempts to mark the page range as dirty. On success, returns the number of current dirty
    /// bytes.
    pub fn mark_dirty<T: PagerBacked>(
        &self,
        page_range: MarkDirtyRange<T>,
    ) -> Result<u64, zx::Status> {
        // Hold an OpenedNode to outlive the lock. When the last OpenedNode is dropped it will call
        // `needs_flush()` which will take the inner lock, which is already held for the duration of
        // this method.
        let _opened_node = page_range.file().dup();

        let mut inner = self.inner.lock();
        if inner.read_only {
            // Enable-verity has already been called on this file.
            page_range.report_failure(zx::Status::BAD_STATE);
            return Err(zx::Status::BAD_STATE);
        }
        let mut new_dirty_pages = DirtyPages::default();
        self.handle.with_overwrite_ranges(|ranges| {
            if let Some(ranges) = ranges {
                for subrange in ranges.overlap(page_range.range()) {
                    // Check the overwrite ranges we have recorded for this file. We only add to the
                    // reservation if the range is not one of our overwrite ranges, since overwrite
                    // ranges are already allocated.
                    match subrange {
                        RangeType::Cow(range) => {
                            new_dirty_pages.reserved += page_count(range);
                        }
                        RangeType::Overwrite(range) => {
                            new_dirty_pages.unreserved += page_count(range);
                        }
                    }
                }
            }
        });
        let mut new_inner = Inner {
            spare: if new_dirty_pages.reserved == 0 { inner.spare } else { SPARE_SIZE },
            ..*inner
        };
        new_inner.dirty_pages += new_dirty_pages;
        let previous_reservation = inner.reservation();
        let new_reservation = new_inner.reservation();
        let reservation_delta = new_reservation - previous_reservation;
        // The reserved amount will never decrease but might be the same.
        let new_reservation = if reservation_delta > 0 {
            match self.allocator().reserve(Some(self.store().store_object_id()), reservation_delta)
            {
                Some(reservation) => Some(reservation),
                None => {
                    page_range.report_failure(zx::Status::NO_SPACE);
                    return Err(zx::Status::NO_SPACE);
                }
            }
        } else {
            None
        };
        page_range.dirty_pages()?;

        // Commit all the changes.
        *inner = new_inner;
        if let Some(reservation) = new_reservation {
            // `PagedObjectHandle` doesn't hold onto a `Reservation` object for tracking
            // reservations. The amount of space reserved by a `PagedObjectHandle` should
            // always be derivable from `Inner`.
            reservation.forget();
        }
        Ok(inner.dirty_pages.total() * PAGE_SIZE)
    }

    /// Queries the VMO to see if it was modified since the last time this function was called.
    fn was_file_modified_since_last_call(&self) -> Result<bool, zx::Status> {
        let stats =
            self.pager().query_vmo_stats(self.vmo(), PagerVmoStatsOptions::RESET_VMO_STATS)?;
        Ok(stats.was_vmo_modified())
    }

    /// Calls `query_dirty_ranges` to collect the ranges of the VMO that need to be flushed.
    fn collect_modified_ranges(&self) -> Result<Vec<VmoDirtyRange>, Error> {
        let mut modified_ranges: Vec<VmoDirtyRange> = Vec::new();
        let vmo = self.vmo();
        let pager = self.pager();

        // Whilst it's tempting to only collect ranges within 0..content_size, we need to collect
        // all the ranges so we can count up how many pages we're not going to flush, and then
        // make sure we return them so that we keep sufficient space reserved.
        let vmo_size = vmo.get_size()?;

        // `query_dirty_ranges` includes both dirty ranges and zero ranges. If there are no zero
        // pages and all of the dirty pages are consecutive then we'll receive only one range back
        // for all of the dirty pages. On the other end, there could be alternating zero and dirty
        // pages resulting in two times the number dirty pages in ranges. Also, since flushing
        // doesn't block mark_dirty, the number of ranges may change as they are being queried. 16
        // ranges was chosen as the initial buffer size to avoid wastefully using memory while also
        // being sufficient for common file usage patterns.
        let mut remaining = 16;
        let mut offset = 0;
        let mut total_received = 0;
        loop {
            modified_ranges.resize(total_received + remaining, VmoDirtyRange::default());
            let actual;
            (actual, remaining) = pager
                .query_dirty_ranges(vmo, offset..vmo_size, &mut modified_ranges[total_received..])
                .context("query_dirty_ranges failed")?;
            total_received += actual;
            // If fewer ranges were received than asked for then drop the extra allocated ranges.
            modified_ranges.resize(total_received, VmoDirtyRange::default());
            if actual == 0 {
                break;
            }
            let last = modified_ranges.last().unwrap();
            offset = last.range().end;
            if remaining == 0 {
                break;
            }
        }
        Ok(modified_ranges)
    }

    /// Queries for the ranges that need to be flushed and splits the ranges into batches that will
    /// each fit into a single transaction.
    fn collect_flush_batches(
        &self,
        content_size: u64,
        flush_type: FlushType,
    ) -> Result<BatchCollectionResult, Error> {
        let page_aligned_content_size = PAGE_SIZE.align_up(content_size).unwrap();
        let modified_ranges =
            self.collect_modified_ranges().context("collect_modified_ranges failed")?;

        debug!(modified_ranges:?, page_aligned_content_size:?; "flush: modified ranges from kernel");

        let mut flush_batches = FlushBatches::new(flush_type);
        let mut last_end = 0;
        for modified_range in modified_ranges {
            // Skip ranges entirely past the stream size.  It might be tempting to consider
            // flushing the range anyway and making up some value for stream size, but that's not
            // safe because the pages will be zeroed before they are written to and it would be
            // wrong to write zeroed data.
            let (range, past_content_size_page_range) =
                modified_range.range().split(page_aligned_content_size);

            if let Some(past_content_size_page_range) = past_content_size_page_range {
                // For now, any data past the end of the content size won't be pre-allocated, so we
                // don't need to consider it when calculating the reservation size. This might
                // change if we support fallocate with the KEEP_SIZE mode which allows for
                // allocations past the end of a file. We also won't be in the middle of an
                // allocation that may have been split into multiple transactions because allocate
                // takes the flush lock.
                if !modified_range.is_zero_range() {
                    // If the range is not zero then space should have been reserved for it that
                    // should continue to be reserved after this flush.
                    flush_batches.skip_range(past_content_size_page_range);
                }
            }

            if let Some(range) = range {
                // Ranges must be returned in order.
                assert!(range.start >= last_end);
                last_end = range.end;
                self.uncached_handle().with_overwrite_ranges(|ranges| {
                    if let Some(ranges) = ranges {
                        for range_chunk in ranges.overlap(range) {
                            let (range, mode) = match range_chunk {
                                RangeType::Cow(range) => (
                                    range,
                                    if modified_range.is_zero_range() {
                                        BatchMode::Zero
                                    } else {
                                        BatchMode::Cow
                                    },
                                ),
                                RangeType::Overwrite(range) => (
                                    range,
                                    if modified_range.is_zero_range() {
                                        BatchMode::Zero
                                    } else {
                                        BatchMode::Overwrite
                                    },
                                ),
                            };
                            flush_batches.add_range(range, mode);
                        }
                    }
                });
            }
        }

        Ok(flush_batches.consume())
    }

    async fn add_metadata_to_transaction<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        content_size: Option<u64>,
        crtime: Option<Timestamp>,
        mtime: Option<Timestamp>,
        ctime: Option<Timestamp>,
    ) -> Result<(), Error> {
        if let Some(content_size) = content_size {
            self.handle.txn_update_size(transaction, content_size, None).await?;
        }
        let attributes = fio::MutableNodeAttributes {
            creation_time: crtime.map(|t| t.as_nanos()),
            modification_time: mtime.map(|t| t.as_nanos()),
            ..Default::default()
        };
        self.handle
            .update_attributes(transaction, Some(&attributes), ctime)
            .await
            .context("update_attributes failed")?;
        Ok(())
    }

    /// Flushes only the metadata of the file by borrowing metadata space.
    async fn flush_metadata(
        &self,
        content_size: u64,
        previous_content_size: u64,
        crtime: Option<Timestamp>,
        mtime: Option<Timestamp>,
    ) -> Result<(), Error> {
        let mut transaction = self.new_transaction(None).await?;
        self.add_metadata_to_transaction(
            &mut transaction,
            if content_size == previous_content_size { None } else { Some(content_size) },
            crtime,
            mtime.clone(),
            mtime,
        )
        .await?;
        transaction.commit().await.context("Failed to commit transaction")?;
        Ok(())
    }

    async fn flush_data(
        &self,
        flush_state: &mut FlushState,
        content_size: u64,
        previous_content_size: u64,
        crtime: Option<Timestamp>,
        mtime: Option<Timestamp>,
        mut flush_batches: Vec<FlushBatch>,
    ) -> Result<(), Error> {
        // We capture the result here because the follow up cannot be done with the scopeguard or
        // any other normal cleanup method because they are all synchronous, while this requires
        // async in order to await the `page_in_barrier()`.
        let res = self
            .flush_data_impl(
                flush_state,
                content_size,
                previous_content_size,
                crtime,
                mtime,
                &mut flush_batches,
            )
            .await;
        // We need to ensure that all page-ins complete before we finish marking the pages as
        // clean. Otherwise the kernel could evict it and allow a page-in to resupply it with
        // stale data. This ensures that any eviction and re-supply comes from a page-in that
        // started after the data was updated and will find up-to-date data.
        if flush_batches.len() > 0 {
            Pager::page_in_barrier().await;
            for batch in flush_batches {
                batch.writeback_end(self.vmo(), self.pager());
            }
        }
        res
    }

    /// `flush_batches will attempt to be flushed, and batches that could not be flushed will be
    /// removed from the set. The ones remaining will need to have `writeback_end()` called on them.
    async fn flush_data_impl(
        &self,
        flush_state: &mut FlushState,
        mut content_size: u64,
        mut previous_content_size: u64,
        crtime: Option<Timestamp>,
        mtime: Option<Timestamp>,
        flush_batches: &mut Vec<FlushBatch>,
    ) -> Result<(), Error> {
        // Drop the batches that don't get finished.
        let mut guard = scopeguard::guard((0, flush_batches), |(num_batches_complete, batches)| {
            batches.truncate(num_batches_complete);
        });
        let (num_batches_complete, batches_to_flush) = &mut *guard;

        for (i, batch) in batches_to_flush.iter().enumerate() {
            let first_batch = i == 0;

            let mut transaction = if batch.mode == BatchMode::Cow {
                self.new_transaction(Some(flush_state.reservation())).await?
            } else {
                self.new_transaction(None).await?
            };
            batch.writeback_begin(self.vmo(), self.pager());

            let size = if batch.end() > content_size {
                // Now that we've called writeback_begin, get the stream size again.  If the
                // stream size has increased (it can't decrease because we hold a lock on
                // truncation), it's possible that it grew before we called writeback_begin in
                // which case, the kernel won't mark the tail page dirty again so we must
                // increase the stream size, but no further than the end of the tail page.
                let new_content_size =
                    self.vmo().get_stream_size().context("get_stream_size failed")?;

                // TODO(https://fxbug.dev/522042546): This might be too strong.  Whilst we do not
                // need to support the case for the file shrinking, it might be possible for a user
                // to make this happen.
                assert!(new_content_size >= content_size);

                content_size = new_content_size;
                Some(std::cmp::min(new_content_size, batch.end()))
            } else if batch.end() > previous_content_size {
                Some(batch.end())
            } else {
                None
            }
            .filter(|s| {
                let changed = *s != previous_content_size;
                previous_content_size = *s;
                changed
            });

            self.add_metadata_to_transaction(
                &mut transaction,
                size,
                if first_batch { crtime } else { None },
                if first_batch { mtime.clone() } else { None },
                if first_batch { mtime } else { None },
            )
            .await?;

            batch
                .add_to_transaction(&mut transaction, &self.vmo, &self.handle, content_size)
                .await
                .context("batch add_to_transaction failed")?;
            transaction.commit().await.context("Failed to commit transaction")?;
            flush_state.did_flush_pages(batch.dirty_pages());

            if first_batch {
                self.inner.lock().end_flush();
            }

            *num_batches_complete += 1;
        }

        Ok(())
    }

    // If `last_chance` is true, pages that are dirty beyond the end of the file are unreserved.
    // This is only safe to do if the file is never going to be flushed again because we will have
    // surrendered the reservation.  We do this so that `needs_flush` returns false upon successful
    // completion, so that we don't log error messages saying dirty data was dropped.
    async fn flush_locked<'a>(
        &self,
        truncate_guard: &TruncateGuard<'a>,
        flush_type: FlushType,
    ) -> Result<(), Error> {
        let pending_shrink = {
            let mut inner = self.inner.lock();
            // Before setting `flushing` to true, double check that a flush is actually required
            // (whilst a lock is held).  We do this because `needs_flush` checks `flushing` and will
            // return `true` whilst we are flushing.
            if !inner.needs_flush() {
                return Ok(());
            }
            assert!(!std::mem::replace(&mut inner.flushing, true));
            inner.pending_shrink
        };

        defer! { self.inner.lock().flushing = false; }

        if let PendingShrink::ShrinkTo(size, update_has_overwrite_extents) = pending_shrink {
            let needs_trim = self
                .shrink_file(size, update_has_overwrite_extents)
                .await
                .context("Failed to shrink file")?;
            self.inner.lock().pending_shrink =
                if needs_trim { PendingShrink::NeedsTrim } else { PendingShrink::None };
        }

        let pending_shrink = self.inner.lock().pending_shrink;
        if let PendingShrink::NeedsTrim = pending_shrink {
            self.store()
                .trim(self.object_id(), truncate_guard)
                .await
                .context("Failed to trim file")?;
            self.inner.lock().pending_shrink = PendingShrink::None;
        }

        // NB: Once the dirty pages are taken, we MUST NOT return without either cleaning them or
        // putting them back, so a scopeguard is added immediately below to ensure that they are
        // managed properly regardless of any future early returns or future cancellation.
        let (mtime, crtime, (dirty_pages, reservation)) = {
            let mut inner = self.inner.lock();
            (
                inner.dirty_mtime.begin_flush(self.was_file_modified_since_last_call()?),
                inner.dirty_crtime.begin_flush(false),
                inner.take(self.allocator(), self.store().store_object_id()),
            )
        };
        let flush_state = FlushState::new(reservation, dirty_pages, flush_type);
        let mut flush_state_wrapper = scopeguard::guard(flush_state, |flush_state| {
            // Can't use a normal drop on FlushState here without letting it hold a reference to the
            // FxVolume in order to report the cleaned pages. If we do that then we lose the ability
            // to isolate the FlushState logic from all the workings of an `FxVolume` and
            // `VolumesDirectory` during testing.
            let cleaned_pages = flush_state.finish(&self.inner);
            if cleaned_pages > 0 {
                self.owner().report_pager_clean(cleaned_pages * PAGE_SIZE);
            }
        });

        #[cfg(test)]
        CALLBACK_BEFORE_RANGE_COLLECTION.call();

        let content_size = self.vmo().get_stream_size().context("get_stream_size failed")?;
        let previous_content_size = self.handle.get_size();

        #[cfg(test)]
        CALLBACK_AFTER_SIZE_CAPTURE.call();

        let BatchCollectionResult {
            batches: flush_batches,
            pages_to_flush: dirty_pages_to_flush,
            pages_not_to_flush: dirty_pages_not_to_flush,
        } = self.collect_flush_batches(content_size, flush_type)?;

        #[cfg(test)]
        CALLBACK_AFTER_RANGE_COLLECTION.call();

        if let Err(_) = flush_state_wrapper
            .set_flush_batch_count(dirty_pages_to_flush, dirty_pages_not_to_flush)
        {
            flush_state_wrapper.take_extra_dirty_pages(&mut *self.inner.lock());
        }

        if flush_batches.is_empty() {
            // If there's no data to flush in a background flush, quit early doing nothing.
            if flush_type == FlushType::Background {
                return Ok(());
            }
            self.flush_metadata(content_size, previous_content_size, crtime, mtime).await?;
            self.inner.lock().end_flush();
            Ok(())
        } else {
            self.flush_data(
                &mut *flush_state_wrapper,
                content_size,
                previous_content_size,
                crtime,
                mtime,
                flush_batches,
            )
            .await
        }
    }

    async fn flush_impl(&self, flush_type: FlushType) -> Result<(), Error> {
        if !self.needs_flush() {
            return Ok(());
        }

        let store = self.handle.store();
        let fs = store.filesystem();
        // If the VMO is shrunk between getting the VMO's size and calling query_dirty_ranges or
        // reading the cached data then the flush could fail. The truncate guard lock is held to
        // prevent the file from shrinking while it's being flushed.
        let truncate_guard =
            fs.truncate_guard(store.store_object_id(), self.handle.object_id()).await;
        self.flush_locked(&truncate_guard, flush_type).await
    }

    pub async fn flush(&self, flush_type: FlushType) -> Result<(), Error> {
        match self.flush_impl(flush_type).await {
            Ok(()) => Ok(()),
            Err(error) => {
                error!(error:?; "Failed to flush");
                Err(error)
            }
        }
    }

    /// Returns true if the file still needs to be trimmed.
    async fn shrink_file(
        &self,
        new_size: u64,
        update_has_overwrite_extents: Option<bool>,
    ) -> Result<bool, Error> {
        let mut transaction = self.new_transaction(None).await?;

        let needs_trim =
            self.handle.shrink(&mut transaction, new_size, update_has_overwrite_extents).await?.0;

        let (mtime, crtime) = {
            let mut inner = self.inner.lock();
            (
                inner.dirty_mtime.begin_flush(self.was_file_modified_since_last_call()?),
                inner.dirty_crtime.begin_flush(false),
            )
        };

        let attributes = fio::MutableNodeAttributes {
            creation_time: crtime.map(|t| t.as_nanos()),
            modification_time: mtime.map(|t| t.as_nanos()),
            ..Default::default()
        };
        // Shrinking the file should also update `change_time` (it'd be the same value as the
        // modification time).
        self.handle
            .update_attributes(&mut transaction, Some(&attributes), mtime)
            .await
            .context("update_attributes failed")?;
        transaction.commit().await.context("Failed to commit transaction")?;
        self.inner.lock().end_flush();

        Ok(needs_trim)
    }

    pub async fn truncate(&self, new_size: u64) -> Result<(), Error> {
        ensure!(new_size <= MAX_FILE_SIZE, FxfsError::InvalidArgs);
        let store = self.handle.store();
        let fs = store.filesystem();
        let _truncate_guard =
            fs.truncate_guard(store.store_object_id(), self.handle.object_id()).await;
        if self.inner.lock().read_only {
            bail!(anyhow!(FxfsError::AccessDenied).context("Cannot truncate verity file"));
        }

        // mark_dirty uses the in-memory tracking of overwrite ranges to decide if it needs to
        // reserve pages or not, so we make sure we update that tracking first thing so we start
        // reserving pages past this size.
        //
        // NB: set_stream_size is pretty unlikely to fail in this scenario (our handle is good, it
        // has the correct rights, we are shrinking so we won't hit any limits), but if it does,
        // this breaks the fallocate contract - all the ranges past this size (rounded up to block
        // size) will be treated as CoW ranges and will start reserving new space. Unfortunately
        // doing it the other way around is worse - there is a potential for mark_dirty calls to
        // come in between set_stream_size and locking inner, and if those pages are in the
        // previously allocated range they won't have reservations for them.
        let update_has_overwrite_extents = self.handle.truncate_overwrite_ranges(new_size)?;

        let vmo = self.vmo.temp_clone();
        // This unblock is to break an executor ordering deadlock situation. Vmo::set_stream_size()
        // may trigger a blocking call back into Fxfs on the same executor via the kernel. If all
        // executor threads are busy, the reentrant call will queue up behind the blocking
        // set_stream_size() call and never complete.
        unblock(move || vmo.set_stream_size(new_size)).await?;

        let previous_content_size = self.handle.get_size();
        let mut inner = self.inner.lock();
        if new_size < previous_content_size {
            inner.pending_shrink = match inner.pending_shrink {
                PendingShrink::None => {
                    PendingShrink::ShrinkTo(new_size, update_has_overwrite_extents)
                }
                PendingShrink::ShrinkTo(size, previous_update) => {
                    let update = update_has_overwrite_extents.or(previous_update);
                    PendingShrink::ShrinkTo(std::cmp::min(size, new_size), update)
                }
                PendingShrink::NeedsTrim => {
                    PendingShrink::ShrinkTo(new_size, update_has_overwrite_extents)
                }
            }
        }

        // Not all paths through the resize method above cause the modification time in the kernel
        // to be set (e.g. if only the stream size is changed), so force an mtime update here.
        let _ = self.was_file_modified_since_last_call()?;
        inner.dirty_mtime = DirtyTimestamp::Some(Timestamp::now());

        // There may be reservations for dirty pages that are no longer relevant but the locations
        // of the pages is not tracked so they are assumed to still be dirty. This will get
        // rectified on the next flush.
        Ok(())
    }

    pub async fn update_attributes(
        &self,
        attributes: &fio::MutableNodeAttributes,
    ) -> Result<(), Error> {
        let empty_attributes = fio::MutableNodeAttributes { ..Default::default() };
        if *attributes == empty_attributes {
            return Ok(());
        }

        // A race condition can occur if another flush occurs between now and the end of the
        // transaction. This lock is to prevent another flush from occurring during that time.
        let fs;
        // The _flush_guard persists until the end of the function
        let _flush_guard;
        let set_creation_time = attributes.creation_time.is_some();
        let set_modification_time = attributes.modification_time.is_some();
        let (attributes_with_pending_mtime, ctime) = {
            let store = self.handle.store();
            fs = store.filesystem();
            let keys =
                lock_keys![LockKey::truncate(store.store_object_id(), self.handle.object_id())];
            _flush_guard = fs.lock_manager().write_lock(keys).await;
            let mut inner = self.inner.lock();
            let mut attributes = attributes.clone();
            // There is an assumption that when we expose ctime and mtime, that ctime is the same
            // as dirty_mtime (when it is some value). When we call `update_attributes(..)`,
            // a situation could arise where ctime is ahead of dirty_mtime and that assumption is no
            // longer true. An example of this is when we call `update_attributes(..)` without
            // setting mtime. In this case, we can no longer assume ctime is equal to dirty_mtime.
            // A way around this is to update attributes with dirty_mtime whenever mtime is not
            // passed in explicitly which will reset dirty_mtime upon successful completion.
            let dirty_mtime = inner
                .dirty_mtime
                .begin_flush(self.was_file_modified_since_last_call()?)
                .map(|t| t.as_nanos());
            if !set_modification_time {
                attributes.modification_time = dirty_mtime;
            }
            (attributes, Some(Timestamp::now()))
        };

        let mut transaction = self.handle.new_transaction().await?;
        self.handle
            .update_attributes(&mut transaction, Some(&attributes_with_pending_mtime), ctime)
            .await
            .context("update_attributes failed")?;
        transaction.commit().await.context("Failed to commit transaction")?;
        // Any changes to the creation_time before this transaction are superseded by the values
        // set in this update.
        {
            let mut inner = self.inner.lock();
            if set_creation_time {
                inner.dirty_crtime = DirtyTimestamp::None;
            }
            // Discard changes to dirty_mtime if no further update was made since begin_flush(..).
            inner.dirty_mtime.end_flush();
        }

        Ok(())
    }

    pub async fn get_properties(&self) -> Result<ObjectProperties, Error> {
        // We must extract information from `inner` *before* we try and retrieve the properties from
        // the handle to avoid a window where we might see old properties.  When we flush, we update
        // the handle and *then* remove the properties from `inner`.
        let (dirty_page_count, data_size, crtime, mtime) = {
            let mut inner = self.inner.lock();

            // If there are no dirty pages, the client can't have modified anything.
            if inner.dirty_pages.total() > 0 && self.was_file_modified_since_last_call()? {
                inner.dirty_mtime = DirtyTimestamp::Some(Timestamp::now());
            }
            (
                inner.dirty_pages.reserved,
                self.vmo.get_stream_size()?,
                inner.dirty_crtime.timestamp(),
                inner.dirty_mtime.timestamp(),
            )
        };
        let mut props = self.handle.get_properties().await?;
        props.allocated_size += dirty_page_count * PAGE_SIZE;
        props.data_attribute_size = data_size;
        if let Some(t) = crtime {
            props.creation_time = t;
        }
        if let Some(t) = mtime {
            props.modification_time = t;
            props.change_time = t;
        }
        Ok(props)
    }

    /// Returns true if the handle needs flushing.
    pub fn needs_flush(&self) -> bool {
        self.inner.lock().needs_flush()
    }

    /// Pre-allocate a region of this file on-disk.
    pub async fn allocate(&self, range: Range<u64>) -> Result<(), Error> {
        if range.start == range.end {
            return Err(anyhow!(FxfsError::InvalidArgs));
        }

        // We want to make sure that flushing, truncate, and allocate are all mutually exclusive,
        // so they all grab the same truncate lock.
        let store = self.store();
        let fs = store.filesystem();
        let truncate_guard = fs.truncate_guard(store.store_object_id(), self.object_id()).await;

        // There are potentially pending shrink operations. We don't particularly care about the
        // performance of allocate, just correctness, so we flush while holding the truncate lock
        // the whole time to make sure the ordering of those operations is correct. Clearing most
        // of the pending write reservations that might overlap with allocated range is a nice side
        // effect, but it's not really required.
        self.flush_locked(&truncate_guard, FlushType::Sync)
            .await
            .inspect_err(|error| error!(error:?; "Failed to flush in allocate"))?;

        // Allocate extends the file if the range is beyond the current file size, so update the
        // stream size in that case as well. Unwrap safe as this can't fail if the vmo is in a valid
        // state.
        if self.vmo.get_stream_size().unwrap() < range.end {
            let vmo = self.vmo.temp_clone();
            // Similar to truncate above, this unblock is to break an executor ordering deadlock
            // situation. Vmo::set_stream_size() may trigger a blocking call back into Fxfs on the
            // same executor via the kernel. If all executor threads are busy, the reentrant call
            // will queue up behind the blocking set_stream_size() call and never complete.
            // It's worth noting that some of the pages marked dirty as part of this call will be
            // COW when the request comes in, but will be overwrite after the `allocate()` call
            // below.
            unblock(move || vmo.set_stream_size(range.end)).await?;
        }

        self.handle.allocate(range).await
    }

    /// Gives up the tracked dirty bytes back to the volume and the reservation to the allocator.
    /// This is done if the file is about to be closed and also deleted, no point in flushing.
    pub fn forget_dirty_pages(&self) {
        self.owner().report_pager_clean(
            self.inner
                .lock()
                .forget_dirty_pages(self.allocator(), self.store().store_object_id())
                .total()
                * PAGE_SIZE,
        );
    }
}

impl Drop for PagedObjectHandle {
    fn drop(&mut self) {
        let mut inner = self.inner.lock();
        // If we're dropping the vmo, all the dirty pages should be flushed, or they should be
        // knowingly discarded using `forget_dirty_pages()`.
        // TODO(https://fxbug.dev/452935329): Turn this into a real assert once we gain some
        // confidence, then delete the cleanup below.
        debug_assert!(inner.dirty_pages.total() == 0, "Dropping VMO with dirty bytes");
        // Return what's left.
        self.owner().report_pager_clean(
            inner.forget_dirty_pages(self.allocator(), self.store().store_object_id()).total()
                * PAGE_SIZE,
        );
    }
}

impl ObjectHandle for PagedObjectHandle {
    fn set_trace(&self, v: bool) {
        self.handle.set_trace(v);
    }
    fn object_id(&self) -> u64 {
        self.handle.object_id()
    }
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.handle.allocate_buffer(size)
    }
    fn block_size(&self) -> BlockSize {
        self.handle.block_size()
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum BatchMode {
    /// Cow pages. Needs to reserve pages to back each page being written.
    Cow,
    /// These do not require space reservations and also does not hold any memory while the page is
    /// dirty. Zero ranges do not generate MarkDirty notifications from the kernel.
    Zero,
    /// Overwrite pages don't need a page reservation as the pages are already allocated.
    Overwrite,
}

#[derive(Debug)]
struct BatchCollectionResult {
    batches: Vec<FlushBatch>,
    pages_to_flush: DirtyPages,
    pages_not_to_flush: DirtyPages,
}

/// Manages the batching of pages to flush per transaction, making sure that each batch stays below
/// FLUSH_BATCH_SIZE, the number of bytes that should be flushed in a single transaction. This
/// prevents transactions from growing larger than can be handled at once.
///
/// This also splits the batches between ranges which should be written using CoW semantics and
/// ranges which should be written using Overwrite semantics, because the transaction options are
/// different.
#[derive(Default, Debug)]
struct FlushBatches {
    batches: Vec<FlushBatch>,
    working_cow_batch: Option<FlushBatch>,
    working_overwrite_batch: Option<FlushBatch>,

    /// The number of dirty pages that will be cleaned as a result of `batches`. This does not
    /// include zero ranges as they are not really dirty pages.
    dirty_pages: DirtyPages,

    /// The number of pages that were marked dirty but are not included in `batches` because they
    /// don't need to be flushed. These are pages that were beyond the VMO's stream size, or are
    /// being left as this is a background flush.
    skipped_dirty_page_count: DirtyPages,

    /// Any zero ranges get put into their own batch. Zero ranges don't actually add any metadata
    /// at the moment (and will error if they do) so we don't need to split them up.
    zero_batch: Option<FlushBatch>,

    /// The type of flush this is for. For background flushes we try to only do full batches and
    /// defer partial batches for later.
    flush_type: FlushType,
}

impl FlushBatches {
    fn new(flush_type: FlushType) -> Self {
        Self { flush_type, ..Default::default() }
    }

    fn add_range(&mut self, range: Range<u64>, mode: BatchMode) {
        let working_batch_ref = match mode {
            BatchMode::Zero => &mut self.zero_batch,
            BatchMode::Cow => &mut self.working_cow_batch,
            BatchMode::Overwrite => &mut self.working_overwrite_batch,
        };
        let mut working_batch = working_batch_ref.get_or_insert_with(|| FlushBatch::new(mode));
        match mode {
            BatchMode::Cow => self.dirty_pages.reserved += page_count(range.clone()),
            BatchMode::Overwrite => self.dirty_pages.unreserved += page_count(range.clone()),
            BatchMode::Zero => {
                // Zero batches do not require any disk writes.
                working_batch.add_range(range);
                return;
            }
        }
        let mut remaining = working_batch.add_range(range);
        while let Some(range) = remaining {
            self.batches.push(working_batch_ref.take().unwrap());
            working_batch = working_batch_ref.get_or_insert_with(|| FlushBatch::new(mode));
            remaining = working_batch.add_range(range);
        }
    }

    /// Skip ranges that are outside the content size.
    fn skip_range(&mut self, range: Range<u64>) {
        // Ranges outside the content size cannot be pre-allocated, and so must have a reservation.
        self.skipped_dirty_page_count.reserved += page_count(range);
    }

    fn consume(mut self) -> BatchCollectionResult {
        if let Some(batch) = self.working_cow_batch {
            if self.flush_type == FlushType::Background && batch.dirty_byte_count < FLUSH_BATCH_SIZE
            {
                let dirty_pages = PAGE_SIZE.align_up_to_blocks(batch.dirty_byte_count);
                self.skipped_dirty_page_count.reserved += dirty_pages;
                self.dirty_pages.reserved -= dirty_pages;
            } else {
                self.batches.push(batch);
            }
        }
        if let Some(batch) = self.working_overwrite_batch {
            if self.flush_type == FlushType::Background && batch.dirty_byte_count < FLUSH_BATCH_SIZE
            {
                let dirty_pages = PAGE_SIZE.align_up_to_blocks(batch.dirty_byte_count);
                self.skipped_dirty_page_count.unreserved += dirty_pages;
                self.dirty_pages.unreserved -= dirty_pages;
            } else {
                self.batches.push(batch);
            }
        }
        if let Some(batch) = self.zero_batch {
            self.batches.push(batch)
        }
        BatchCollectionResult {
            batches: self.batches,
            pages_to_flush: self.dirty_pages,
            pages_not_to_flush: self.skipped_dirty_page_count,
        }
    }
}

#[derive(Debug, PartialEq)]
struct FlushBatch {
    /// The ranges to be flushed in this batch.
    ranges: Vec<Range<u64>>,

    /// The number of bytes spanned by `ranges`, excluding zero ranges if this is a CoW batch.
    dirty_byte_count: u64,

    /// The mode of this batch.
    mode: BatchMode,
}

impl FlushBatch {
    fn new(mode: BatchMode) -> Self {
        Self { ranges: Vec::new(), dirty_byte_count: 0, mode }
    }

    /// Adds `range` to this batch. If `range` doesn't entirely fit into this batch then the
    /// remaining part of the range is returned.
    fn add_range(&mut self, range: Range<u64>) -> Option<Range<u64>> {
        debug_assert!(range.start >= self.ranges.last().map_or(0, |r| r.end));
        if self.mode == BatchMode::Zero {
            self.ranges.push(range);
            return None;
        }

        let split_point = range.start + (FLUSH_BATCH_SIZE - self.dirty_byte_count);
        let (range, remaining) = range.split(split_point);

        if let Some(range) = range {
            self.dirty_byte_count += range.end - range.start;
            self.ranges.push(range);
        }

        remaining
    }

    /// The number of dirty pages that this batch covers, separated by reserved and unreserved based
    ///  on the batch mode.
    fn dirty_pages(&self) -> DirtyPages {
        match self.mode {
            BatchMode::Cow => DirtyPages {
                reserved: PAGE_SIZE.align_up_to_blocks(self.dirty_byte_count),
                unreserved: 0,
            },
            BatchMode::Overwrite => DirtyPages {
                reserved: 0,
                unreserved: PAGE_SIZE.align_up_to_blocks(self.dirty_byte_count),
            },
            BatchMode::Zero => DirtyPages::default(),
        }
    }

    fn writeback_begin(&self, vmo: &zx::Vmo, pager: &Pager) {
        let options = if self.mode == BatchMode::Zero {
            zx::PagerWritebackBeginOptions::DIRTY_RANGE_IS_ZERO
        } else {
            zx::PagerWritebackBeginOptions::empty()
        };
        for range in &self.ranges {
            pager.writeback_begin(vmo, range.clone(), options);
        }
    }

    fn writeback_end(&self, vmo: &zx::Vmo, pager: &Pager) {
        for range in &self.ranges {
            pager.writeback_end(vmo, range.clone());
        }
    }

    async fn add_to_transaction<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        vmo: &zx::Vmo,
        handle: &'a DataObjectHandle<FxVolume>,
        content_size: u64,
    ) -> Result<(), Error> {
        if self.mode == BatchMode::Zero {
            for range in &self.ranges {
                // TODO(https://fxbug.dev/349447236): This doesn't seem to ever do anything, so
                // this experimental assert is going to sit around for a bit to see if there is a
                // case we aren't aware of.
                assert!(handle.check_unwritten_zero(range.clone()).await?);
            }
            return Ok(());
        }

        if self.dirty_byte_count > 0 {
            let mut buffer =
                handle.allocate_buffer(self.dirty_byte_count.try_into().unwrap()).await;
            let mut slice = buffer.as_mut();

            let mut dirty_ranges = Vec::new();
            for range in &self.ranges {
                let range = range.clone();
                let len =
                    (std::cmp::min(range.end, content_size) - range.start).try_into().unwrap();
                let (mut head, tail) = slice.split_at_mut(len);

                // SAFETY: The device-backed buffer is valid for writes up to `len` bytes.
                // Using read_raw performs a direct VMO-to-pointer copy without creating
                // standard Rust &mut [u8] references, avoiding any compiler aliasing UB.
                unsafe {
                    vmo.read_raw(head.as_mut_ptr(), len, range.start)?;
                }

                slice = tail;

                // Zero out the tail.
                if range.end > content_size {
                    let (mut head, tail) = slice.split_at_mut((range.end - content_size) as usize);
                    head.fill(0);
                    slice = tail;
                }
                dirty_ranges.push(range);
            }
            match self.mode {
                BatchMode::Overwrite => handle
                    .multi_overwrite(transaction, AttributeId::DATA, &dirty_ranges, buffer.as_mut())
                    .await
                    .context("multi_overwrite failed")?,
                BatchMode::Cow => handle
                    .multi_write(transaction, AttributeId::DATA, &dirty_ranges, buffer.as_mut())
                    .await
                    .context("multi_write failed")?,
                BatchMode::Zero => unreachable!("Handled above"),
            }
        }

        Ok(())
    }

    fn end(&self) -> u64 {
        self.ranges.last().map(|r| r.end).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuchsia::directory::FxDirectory;
    use crate::fuchsia::file::FxFile;
    use crate::fuchsia::node::FxNode;
    use crate::fuchsia::testing::{
        TestFixture, TestFixtureOptions, close_dir_checked, close_file_checked, open_file_checked,
    };
    use crate::fuchsia::volume::{FxVolumeAndRoot, MemoryPressureConfig};
    use anyhow::bail;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use fuchsia_fs::file;
    use futures::join;
    use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
    use fxfs::object_store::NewChildStoreOptions;
    use fxfs::object_store::volume::root_volume;
    use refaults_vmo::PageRefaultCounter;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
    use std::sync::{Weak, mpsc};
    use std::time::Duration;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use test_util::{assert_geq, assert_lt};

    const BLOCK_SIZE: u32 = 512;
    const BLOCK_COUNT: u64 = 16384;
    const FILE_NAME: &str = "file";
    const ONE_DAY: u64 = Duration::from_secs(60 * 60 * 24).as_nanos() as u64;

    async fn get_attributes_checked(
        file: &fio::FileProxy,
        query: fio::NodeAttributesQuery,
    ) -> fio::NodeAttributes2 {
        let (mutable_attributes, immutable_attributes) = file
            .get_attributes(query)
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("get_attributes failed");
        fio::NodeAttributes2 { mutable_attributes, immutable_attributes }
    }

    async fn update_attributes_checked(
        file: &fio::FileProxy,
        attributes: &fio::MutableNodeAttributes,
    ) {
        file.update_attributes(&attributes)
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("update_attributes failed");
    }

    async fn open_filesystem() -> (OpenFxFilesystem, FxVolumeAndRoot) {
        open_filesystem_with_hooks(Default::default()).await
    }

    async fn open_filesystem_with_hooks(
        hooks: Arc<fxfs::hooks::HooksHandle>,
    ) -> (OpenFxFilesystem, FxVolumeAndRoot) {
        let device = DeviceHolder::new(FakeDevice::new(BLOCK_COUNT, BLOCK_SIZE));
        let fs = FxFilesystemBuilder::new().hooks(hooks).format(true).open(device).await.unwrap();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let store = root_volume.new_volume("vol", NewChildStoreOptions::default()).await.unwrap();
        let store_object_id = store.store_object_id();
        let volume = FxVolumeAndRoot::new::<FxDirectory>(
            Weak::new(),
            store,
            store_object_id,
            "vol".to_owned(),
            Arc::new(PageRefaultCounter::new().unwrap()),
            MemoryPressureConfig::default(),
        )
        .await
        .unwrap();
        (fs, volume)
    }

    fn open_volume(volume: &FxVolumeAndRoot) -> fio::DirectoryProxy {
        let (root, server_end) = create_proxy::<fio::DirectoryMarker>();
        volume.root().clone().serve(fio::PERM_READABLE | fio::PERM_WRITABLE, server_end);
        root
    }

    #[fuchsia::test]
    async fn test_large_flush_requiring_multiple_transactions() {
        let transaction_count = AtomicU64::new(0);
        let (mut hooks, fs_hooks) = fxfs::hooks::Hooks::new();
        hooks.set_pre_commit(|_| {
            transaction_count.fetch_add(1, Ordering::Relaxed);
            Ok::<(), anyhow::Error>(())
        });
        let (fs, volume) = open_filesystem_with_hooks(fs_hooks).await;
        let root = open_volume(&volume);

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let stream = file.describe().await.unwrap().stream.unwrap();
        let file_id =
            file.get_attributes(fio::NodeAttributesQuery::ID).await.unwrap().unwrap().1.id.unwrap();
        // Block background flushes by holding the truncate lock.
        let truncate_guard =
            fs.truncate_guard(volume.volume().store().store_object_id(), file_id).await;

        // Touch enough pages that 3 transaction will be required.
        unblock(move || {
            let write_count: u64 = (FLUSH_BATCH_SIZE / PAGE_SIZE) * 2 + 10;
            for i in 0..write_count {
                stream
                    .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[0, 1, 2, 3, 4])
                    .expect("write should succeed");
            }
        })
        .await;

        transaction_count.store(0, Ordering::Relaxed);
        // Let go of the truncation guard. This will let the background flush complete, and this
        // sync.
        std::mem::drop(truncate_guard);
        file.sync().await.unwrap().unwrap();
        assert_eq!(transaction_count.load(Ordering::Relaxed), 3);

        close_file_checked(file).await;
        close_dir_checked(root).await;
        volume.volume().terminate().await;
        fs.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_multi_transaction_flush_with_failing_middle_transaction() {
        let fail_transaction_after = AtomicI64::new(i64::MAX);
        let (mut hooks, fs_hooks) = fxfs::hooks::Hooks::new();
        hooks.set_pre_commit(|_| {
            if fail_transaction_after.fetch_sub(1, Ordering::Relaxed) < 1 {
                bail!("Intentionally fail transaction")
            } else {
                Ok(())
            }
        });
        let (fs, volume) = open_filesystem_with_hooks(fs_hooks).await;
        let root = open_volume(&volume);

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let stream = file.describe().await.unwrap().stream.unwrap();
        let file_id =
            file.get_attributes(fio::NodeAttributesQuery::ID).await.unwrap().unwrap().1.id.unwrap();
        // Block background flushes by holding the truncate lock.
        let truncate_guard =
            fs.truncate_guard(volume.volume().store().store_object_id(), file_id).await;
        // Touch enough pages that 3 transaction will be required.
        unblock(move || {
            let write_count: u64 = (FLUSH_BATCH_SIZE / PAGE_SIZE) * 2 + 10;
            for i in 0..write_count {
                stream
                    .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &i.to_le_bytes())
                    .expect("write should succeed");
            }
        })
        .await;

        // Succeed the multi_write call from the first transaction and fail the multi_write call
        // from the second transaction. The metadata from all of the transactions doesn't get
        // written to disk until the journal is synced which happens in FxFile::sync after all of
        // the multi_writes.
        fail_transaction_after.store(1, Ordering::Relaxed);
        // Let go of the truncation guard. This will let the background flush complete, and this
        // sync.
        std::mem::drop(truncate_guard);
        file.sync().await.unwrap().expect_err("sync should fail");
        fail_transaction_after.store(i64::MAX, Ordering::Relaxed);

        // This sync will panic if the allocator reservations intended for the second or third
        // transactions weren't retained or the pages in the first transaction weren't properly
        // cleaned.
        file.sync().await.unwrap().expect("sync should succeed");

        close_file_checked(file).await;
        close_dir_checked(root).await;
        volume.volume().terminate().await;
        fs.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_writeback_begin_and_end_are_called_correctly() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let info = file.describe().await.expect("describe failed");
        let stream = Arc::new(info.stream.unwrap());

        let write_count: u64 = (FLUSH_BATCH_SIZE / PAGE_SIZE) * 2 + 10;

        {
            let stream = stream.clone();
            unblock(move || {
                // Dirty lots of pages so multiple transactions are required.
                for i in 0..(write_count * 2) {
                    stream
                        .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[0, 1, 2, 3, 4])
                        .unwrap();
                }
            })
            .await;
        }
        // Sync the file to mark all of pages as clean.
        file.sync().await.unwrap().unwrap();
        // Set the file size to 0 to mark all of the cleaned pages as zero pages.
        file.resize(0).await.unwrap().unwrap();

        {
            let stream = stream.clone();
            unblock(move || {
                // Write to every other page to force alternating zero and dirty pages.
                for i in 0..write_count {
                    stream
                        .write_at(
                            zx::StreamWriteOptions::empty(),
                            i * PAGE_SIZE * 2,
                            &[0, 1, 2, 3, 4],
                        )
                        .unwrap();
                }
            })
            .await;
        }
        // Sync to mark everything as clean again.
        file.sync().await.unwrap().unwrap();

        // Touch a single page so another flush is required.
        unblock(move || {
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[0, 1, 2, 3, 4]).unwrap()
        })
        .await;

        // If writeback_begin and writeback_end weren't called in the correct order in the previous
        // sync then not all of the pages will have been marked clean. If not all of the pages were
        // cleaned then this sync will panic because there won't be enough reserved space to clean
        // the pages that weren't properly cleaned in the previous sync.
        file.sync().await.unwrap().unwrap();

        close_file_checked(file).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_writing_overrides_set_mtime() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::MODIFICATION_TIME).await;
        let initial_time = node_attrs.mutable_attributes.modification_time.unwrap();

        // Advance the mtime by a large amount that should be reachable by the test.
        update_attributes_checked(
            &file,
            &fio::MutableNodeAttributes {
                modification_time: Some(initial_time + ONE_DAY),
                ..Default::default()
            },
        )
        .await;

        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::MODIFICATION_TIME).await;

        let updated_time = node_attrs.mutable_attributes.modification_time.unwrap();
        assert!(updated_time > initial_time);

        file::write(&file, &[1, 2, 3, 4]).await.expect("write failed");

        // Writing to the file after advancing the mtime will bring the mtime back to the current
        // time.
        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::MODIFICATION_TIME).await;
        let current_mtime = node_attrs.mutable_attributes.modification_time.unwrap();

        assert!(current_mtime < updated_time);

        file.sync().await.unwrap().unwrap();
        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::MODIFICATION_TIME).await;
        let synced_mtime = node_attrs.mutable_attributes.modification_time.unwrap();

        assert_eq!(synced_mtime, current_mtime);

        close_file_checked(file).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_flushing_after_get_attr_does_not_change_mtime() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.write(&[1, 2, 3, 4])
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw)
            .expect("write failed");

        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::MODIFICATION_TIME).await;
        let first_mtime = node_attrs.mutable_attributes.modification_time.unwrap();

        // The contents of the file haven't changed since get_attr was called so the flushed mtime
        // should be the same as the mtime returned from the get_attr call.
        file.sync().await.unwrap().unwrap();
        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::MODIFICATION_TIME).await;
        let flushed_mtime = node_attrs.mutable_attributes.modification_time.unwrap();
        assert_eq!(flushed_mtime, first_mtime);

        close_file_checked(file).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_timestamps_are_preserved_across_flush_failures() {
        let fail_transaction = AtomicBool::new(false);
        let (mut hooks, fs_hooks) = fxfs::hooks::Hooks::new();
        hooks.set_pre_commit(|_| {
            if fail_transaction.load(Ordering::Relaxed) {
                Err(zx::Status::IO).context("Intentionally fail transaction")
            } else {
                Ok(())
            }
        });
        let (fs, volume) = open_filesystem_with_hooks(fs_hooks).await;
        let root = open_volume(&volume);

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        file::write(&file, [1, 2, 3, 4]).await.unwrap();

        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::CREATION_TIME).await;
        let creation_time = node_attrs.mutable_attributes.creation_time.unwrap();
        let future = creation_time + ONE_DAY;
        update_attributes_checked(
            &file,
            &fio::MutableNodeAttributes {
                creation_time: Some(future),
                modification_time: Some(future),
                ..Default::default()
            },
        )
        .await;

        fail_transaction.store(true, Ordering::Relaxed);
        file.sync().await.unwrap().expect_err("sync should fail");
        fail_transaction.store(false, Ordering::Relaxed);

        let node_attrs = get_attributes_checked(
            &file,
            fio::NodeAttributesQuery::CREATION_TIME | fio::NodeAttributesQuery::MODIFICATION_TIME,
        )
        .await;
        assert_eq!(node_attrs.mutable_attributes.creation_time.unwrap(), future);
        assert_eq!(node_attrs.mutable_attributes.modification_time.unwrap(), future);

        close_file_checked(file).await;
        close_dir_checked(root).await;
        volume.volume().terminate().await;
        fs.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_max_file_size() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let info = file.describe().await.unwrap();
        let stream: zx::Stream = info.stream.unwrap();

        unblock(move || {
            stream
                .write_at(zx::StreamWriteOptions::empty(), MAX_FILE_SIZE - 1, &[1])
                .expect("write should succeed");
            stream
                .write_at(zx::StreamWriteOptions::empty(), MAX_FILE_SIZE, &[1])
                .expect_err("write should fail");
        })
        .await;

        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::CONTENT_SIZE).await;
        assert_eq!(node_attrs.immutable_attributes.content_size.unwrap(), MAX_FILE_SIZE);

        file.resize(MAX_FILE_SIZE).await.unwrap().expect("resize should succeed");
        file.resize(MAX_FILE_SIZE + 1).await.unwrap().expect_err("resize should fail");

        close_file_checked(file).await;
        fixture.close().await;
    }

    #[test]
    fn test_reservation_needed() {
        assert_eq!(FLUSH_BATCH_SIZE / PAGE_SIZE, 128);

        assert_eq!(reservation_needed(0), 0);

        assert_eq!(reservation_needed(1), TRANSACTION_METADATA_MAX_AMOUNT + 1 * PAGE_SIZE);
        assert_eq!(reservation_needed(10), TRANSACTION_METADATA_MAX_AMOUNT + 10 * PAGE_SIZE);
        assert_eq!(reservation_needed(128), TRANSACTION_METADATA_MAX_AMOUNT + 128 * PAGE_SIZE);

        assert_eq!(reservation_needed(129), 2 * TRANSACTION_METADATA_MAX_AMOUNT + 129 * PAGE_SIZE);
        assert_eq!(reservation_needed(256), 2 * TRANSACTION_METADATA_MAX_AMOUNT + 256 * PAGE_SIZE);

        assert_eq!(
            reservation_needed(1500),
            12 * TRANSACTION_METADATA_MAX_AMOUNT + 1500 * PAGE_SIZE
        );
    }

    #[test]
    fn test_flush_batch_dirty_pages() {
        {
            let mut flush_batch = FlushBatch::new(BatchMode::Cow);
            assert_eq!(flush_batch.dirty_pages().reserved, 0);
            assert_eq!(flush_batch.dirty_pages().unreserved, 0);

            flush_batch.add_range(4096..8192);
            assert_eq!(flush_batch.dirty_pages().reserved, 1);

            // Adding a partial page rounds up to the next page. Only the page containing the
            // content size should be a partial page so handling multiple partial pages isn't
            // necessary.
            flush_batch.add_range(8192..8704);
            assert_eq!(flush_batch.dirty_pages().reserved, 2);
            assert_eq!(flush_batch.dirty_pages().unreserved, 0);
        }

        {
            let mut flush_batch = FlushBatch::new(BatchMode::Overwrite);
            assert_eq!(flush_batch.dirty_pages().reserved, 0);
            assert_eq!(flush_batch.dirty_pages().unreserved, 0);

            flush_batch.add_range(4096..8192);
            assert_eq!(flush_batch.dirty_pages().reserved, 0);
            assert_eq!(flush_batch.dirty_pages().unreserved, 1);
        }

        let mut flush_batch = FlushBatch::new(BatchMode::Zero);
        assert_eq!(flush_batch.dirty_pages().reserved, 0);
        assert_eq!(flush_batch.dirty_pages().unreserved, 0);

        flush_batch.add_range(4096..8192);
        assert_eq!(flush_batch.dirty_pages().reserved, 0);
        assert_eq!(flush_batch.dirty_pages().unreserved, 0);
    }

    #[test]
    fn test_flush_batch_add_range_splits_range() {
        let mut flush_batch = FlushBatch::new(BatchMode::Cow);

        let remaining = flush_batch.add_range(0..(FLUSH_BATCH_SIZE + 4096));
        let remaining = remaining.expect("The batch should have run out of space");
        assert_eq!(remaining, FLUSH_BATCH_SIZE..(FLUSH_BATCH_SIZE + 4096));

        let range = (FLUSH_BATCH_SIZE + 4096)..(FLUSH_BATCH_SIZE + 8192);
        assert_eq!(flush_batch.add_range(range.clone()), Some(range));
    }

    #[test]
    fn test_flush_batches_add_range_huge_range() {
        let mut batches = FlushBatches::default();
        batches.add_range(0..(FLUSH_BATCH_SIZE * 2 + 8192), BatchMode::Cow);
        let BatchCollectionResult {
            batches,
            pages_to_flush: dirty_page_count,
            pages_not_to_flush: skipped_dirty_page_count,
        } = batches.consume();
        assert_eq!(dirty_page_count.reserved, 258);
        assert_eq!(skipped_dirty_page_count.total(), 0);
        assert_eq!(
            batches,
            vec![
                FlushBatch {
                    ranges: vec![0..FLUSH_BATCH_SIZE],
                    dirty_byte_count: FLUSH_BATCH_SIZE,
                    mode: BatchMode::Cow,
                },
                FlushBatch {
                    ranges: vec![FLUSH_BATCH_SIZE..(FLUSH_BATCH_SIZE * 2)],
                    dirty_byte_count: FLUSH_BATCH_SIZE,
                    mode: BatchMode::Cow,
                },
                FlushBatch {
                    ranges: vec![(FLUSH_BATCH_SIZE * 2)..(FLUSH_BATCH_SIZE * 2 + 8192)],
                    dirty_byte_count: 8192,
                    mode: BatchMode::Cow,
                }
            ]
        );
    }

    #[test]
    fn test_flush_cow_batches_background() {
        let mut batches = FlushBatches::new(FlushType::Background);
        batches.add_range(0..(FLUSH_BATCH_SIZE * 2 + PAGE_SIZE * 2), BatchMode::Cow);
        let BatchCollectionResult {
            batches,
            pages_to_flush: dirty_page_count,
            pages_not_to_flush: skipped_dirty_page_count,
        } = batches.consume();
        assert_eq!(dirty_page_count.reserved, FLUSH_BATCH_SIZE * 2 / PAGE_SIZE);
        assert_eq!(skipped_dirty_page_count.reserved, 2);
        assert_eq!(
            batches,
            vec![
                FlushBatch {
                    ranges: vec![0..FLUSH_BATCH_SIZE],
                    dirty_byte_count: FLUSH_BATCH_SIZE,
                    mode: BatchMode::Cow,
                },
                FlushBatch {
                    ranges: vec![FLUSH_BATCH_SIZE..(FLUSH_BATCH_SIZE * 2)],
                    dirty_byte_count: FLUSH_BATCH_SIZE,
                    mode: BatchMode::Cow,
                },
            ]
        );
    }

    #[test]
    fn test_flush_overwrite_batches_background() {
        let mut batches = FlushBatches::new(FlushType::Background);
        batches.add_range(0..(FLUSH_BATCH_SIZE + PAGE_SIZE), BatchMode::Overwrite);
        let BatchCollectionResult {
            batches,
            pages_to_flush: dirty_page_count,
            pages_not_to_flush: skipped_dirty_page_count,
        } = batches.consume();
        assert_eq!(dirty_page_count.unreserved, FLUSH_BATCH_SIZE / PAGE_SIZE);
        // We don't count overwrite pages here, they don't need reservations.
        assert_eq!(skipped_dirty_page_count.unreserved, 1);
        assert_eq!(
            batches,
            vec![FlushBatch {
                ranges: vec![0..FLUSH_BATCH_SIZE],
                dirty_byte_count: FLUSH_BATCH_SIZE,
                mode: BatchMode::Overwrite,
            }]
        );
    }

    #[test]
    fn test_flush_one_full_batch_background() {
        let mut batches = FlushBatches::new(FlushType::Background);
        batches.add_range(0..FLUSH_BATCH_SIZE, BatchMode::Cow);
        let BatchCollectionResult {
            batches,
            pages_to_flush: dirty_page_count,
            pages_not_to_flush: skipped_dirty_page_count,
        } = batches.consume();
        assert_eq!(dirty_page_count.reserved, FLUSH_BATCH_SIZE / PAGE_SIZE);
        assert_eq!(skipped_dirty_page_count.reserved, 0);
        assert_eq!(
            batches,
            vec![FlushBatch {
                ranges: vec![0..FLUSH_BATCH_SIZE],
                dirty_byte_count: FLUSH_BATCH_SIZE,
                mode: BatchMode::Cow,
            },]
        );
    }

    #[test]
    fn test_flush_batches_background_drops_last_pages() {
        let mut batches = FlushBatches::new(FlushType::Background);
        // Despite having better chunking, it will drop the last pages not the smaller ranges.
        // This matters since part of the goal is not to get in the way of linear writers.
        batches.add_range(0..(FLUSH_BATCH_SIZE - PAGE_SIZE * 2), BatchMode::Cow);
        batches.add_range(FLUSH_BATCH_SIZE..(FLUSH_BATCH_SIZE * 2), BatchMode::Cow);
        let BatchCollectionResult {
            batches,
            pages_to_flush: dirty_page_count,
            pages_not_to_flush: skipped_dirty_page_count,
        } = batches.consume();
        assert_eq!(dirty_page_count.reserved, FLUSH_BATCH_SIZE / PAGE_SIZE);
        assert_eq!(skipped_dirty_page_count.reserved, (FLUSH_BATCH_SIZE / PAGE_SIZE) - 2);
        assert_eq!(
            batches,
            vec![FlushBatch {
                ranges: vec![
                    0..(FLUSH_BATCH_SIZE - PAGE_SIZE * 2),
                    FLUSH_BATCH_SIZE..(FLUSH_BATCH_SIZE + PAGE_SIZE * 2)
                ],
                dirty_byte_count: FLUSH_BATCH_SIZE,
                mode: BatchMode::Cow,
            },]
        );
    }

    #[test]
    fn test_flush_batches_add_range_multiple_ranges() {
        let mut batches = FlushBatches::default();
        batches.add_range(0..PAGE_SIZE.get(), BatchMode::Cow);
        batches.add_range(PAGE_SIZE.get()..(PAGE_SIZE * 3), BatchMode::Zero);
        batches.add_range((PAGE_SIZE * 7)..(PAGE_SIZE * 150), BatchMode::Cow);
        batches.add_range((PAGE_SIZE * 200)..(PAGE_SIZE * 500), BatchMode::Zero);
        batches.add_range((PAGE_SIZE * 500)..(PAGE_SIZE * 650), BatchMode::Overwrite);

        let BatchCollectionResult {
            batches,
            pages_to_flush: dirty_page_count,
            pages_not_to_flush: skipped_dirty_page_count,
        } = batches.consume();
        assert_eq!(dirty_page_count.reserved, 144);
        assert_eq!(dirty_page_count.unreserved, 150);
        assert_eq!(skipped_dirty_page_count.total(), 0);
        assert_eq!(
            batches,
            vec![
                FlushBatch {
                    ranges: vec![0..PAGE_SIZE.get(), (PAGE_SIZE * 7)..(PAGE_SIZE * 134)],
                    dirty_byte_count: FLUSH_BATCH_SIZE,
                    mode: BatchMode::Cow,
                },
                FlushBatch {
                    ranges: vec![(PAGE_SIZE * 500)..(PAGE_SIZE * 628)],
                    dirty_byte_count: FLUSH_BATCH_SIZE,
                    mode: BatchMode::Overwrite,
                },
                FlushBatch {
                    ranges: vec![(PAGE_SIZE * 134)..(PAGE_SIZE * 150),],
                    dirty_byte_count: 16 * PAGE_SIZE,
                    mode: BatchMode::Cow,
                },
                FlushBatch {
                    ranges: vec![(PAGE_SIZE * 628)..(PAGE_SIZE * 650),],
                    dirty_byte_count: 22 * PAGE_SIZE,
                    mode: BatchMode::Overwrite,
                },
                FlushBatch {
                    ranges: vec![
                        PAGE_SIZE.get()..(PAGE_SIZE * 3),
                        (PAGE_SIZE * 200)..(PAGE_SIZE * 500)
                    ],
                    dirty_byte_count: 0,
                    mode: BatchMode::Zero,
                },
            ]
        );
    }

    #[test]
    fn test_flush_batches_skip_range() {
        let mut batches = FlushBatches::default();
        batches.skip_range(0..8192);
        let BatchCollectionResult {
            batches,
            pages_to_flush: dirty_page_count,
            pages_not_to_flush: skipped_dirty_page_count,
        } = batches.consume();
        assert_eq!(dirty_page_count.reserved, 0);
        assert_eq!(batches, Vec::new());
        assert_eq!(skipped_dirty_page_count.reserved, 2);
    }

    #[fuchsia::test]
    async fn test_retry_shrink_transaction() {
        let fail_transaction = AtomicBool::new(false);
        let (mut hooks, fs_hooks) = fxfs::hooks::Hooks::new();
        hooks.set_pre_commit(|_| {
            if fail_transaction.load(Ordering::Relaxed) {
                bail!("Intentionally fail transaction")
            } else {
                Ok(())
            }
        });
        let (fs, volume) = open_filesystem_with_hooks(fs_hooks).await;
        let root = open_volume(&volume);

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let initial_file_size = PAGE_SIZE.get() as usize * 10;
        file::write(&file, vec![5u8; initial_file_size]).await.unwrap();
        file.sync().await.unwrap().map_err(zx::ok).unwrap();

        let initial_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::STORAGE_SIZE).await;
        let initial_storage_size = initial_attrs.immutable_attributes.storage_size.unwrap();

        assert_geq!(initial_storage_size, initial_file_size as u64);
        file.resize(0).await.unwrap().map_err(zx::ok).unwrap();

        fail_transaction.store(true, Ordering::Relaxed);
        file.sync().await.unwrap().expect_err("flush should have failed");
        fail_transaction.store(false, Ordering::Relaxed);

        // Verify that the file wasn't resized and non of the blocks were freed.
        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::STORAGE_SIZE).await;
        assert_eq!(node_attrs.immutable_attributes.storage_size.unwrap(), initial_storage_size,);

        file.sync().await.unwrap().map_err(zx::ok).unwrap();
        let node_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::STORAGE_SIZE).await;
        // The shrink transaction was retried and the blocks were freed.
        assert_eq!(node_attrs.immutable_attributes.storage_size.unwrap(), 0);

        close_file_checked(file).await;
        close_dir_checked(root).await;
        volume.volume().terminate().await;
        fs.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_retry_trim_transaction() {
        let fail_transaction_after = AtomicI64::new(i64::MAX);
        let (mut hooks, fs_hooks) = fxfs::hooks::Hooks::new();
        hooks.set_pre_commit(|_| {
            if fail_transaction_after.fetch_sub(1, Ordering::Relaxed) < 1 {
                bail!("Intentionally fail transaction")
            } else {
                Ok(())
            }
        });
        let (fs, volume) = open_filesystem_with_hooks(fs_hooks).await;
        let root = open_volume(&volume);

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        // Write to every other page to generate lots of small extents that will require multiple
        // transactions to be freed.
        let write_count: u64 = 256;
        for i in 0..write_count {
            file.write_at(&[5u8; 1], PAGE_SIZE * 2 * i)
                .await
                .unwrap()
                .map_err(zx::ok)
                .unwrap_or_else(|e| panic!("Write {} failed {:?}", i, e));
        }
        file.sync().await.unwrap().map_err(zx::ok).unwrap();
        let initial_attrs =
            get_attributes_checked(&file, fio::NodeAttributesQuery::STORAGE_SIZE).await;
        let initial_storage_size = initial_attrs.immutable_attributes.storage_size.unwrap();

        assert_geq!(initial_storage_size, write_count * PAGE_SIZE);
        file.resize(0).await.unwrap().map_err(zx::ok).unwrap();

        // Allow the shrink transaction, fail the trim transaction.
        fail_transaction_after.store(1, Ordering::Relaxed);
        file.sync().await.unwrap().expect_err("flush should have failed");
        fail_transaction_after.store(i64::MAX, Ordering::Relaxed);

        // Some of the extents will be freed by the shrink transactions but not all of them.
        let attrs = get_attributes_checked(&file, fio::NodeAttributesQuery::STORAGE_SIZE).await;
        assert_ne!(attrs.immutable_attributes.storage_size.unwrap(), 0);
        assert_lt!(attrs.immutable_attributes.storage_size.unwrap(), initial_storage_size);

        file.sync().await.unwrap().map_err(zx::ok).unwrap();
        let attrs = get_attributes_checked(&file, fio::NodeAttributesQuery::STORAGE_SIZE).await;
        // The trim transaction was retried and the extents were freed.
        assert_eq!(attrs.immutable_attributes.storage_size.unwrap(), 0);

        close_file_checked(file).await;
        close_dir_checked(root).await;
        volume.volume().terminate().await;
        fs.close().await.expect("close filesystem failed");
    }

    // Growing the file isn't tracked by `truncate` and if it's to a page boundary then the
    // kernel won't mark a page as dirty.
    #[fuchsia::test]
    async fn test_needs_flush_after_growing_file_to_page_boundary() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        file.resize(PAGE_SIZE.get()).await.unwrap().map_err(zx::ok).unwrap();
        close_file_checked(file).await;

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let attrs = get_attributes_checked(&file, fio::NodeAttributesQuery::CONTENT_SIZE).await;
        assert_eq!(attrs.immutable_attributes.content_size.unwrap(), PAGE_SIZE);

        close_file_checked(file).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_get_update_attrs_and_attributes_parity() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let now = Timestamp::now().as_nanos();
        update_attributes_checked(
            &file,
            &fio::MutableNodeAttributes {
                creation_time: Some(now),
                modification_time: Some(now - ONE_DAY),
                mode: Some(111),
                gid: Some(222),
                ..Default::default()
            },
        )
        .await;
        let updated_attributes = get_attributes_checked(
            &file,
            fio::NodeAttributesQuery::CREATION_TIME
                | fio::NodeAttributesQuery::MODIFICATION_TIME
                | fio::NodeAttributesQuery::MODE
                | fio::NodeAttributesQuery::GID,
        )
        .await;
        let mut expected_attributes = fio::NodeAttributes2 {
            mutable_attributes: fio::MutableNodeAttributes { ..Default::default() },
            immutable_attributes: fio::ImmutableNodeAttributes { ..Default::default() },
        };
        expected_attributes.mutable_attributes.creation_time = Some(now);
        // modification_time should reflect the latest change
        expected_attributes.mutable_attributes.modification_time = Some(now - ONE_DAY);
        expected_attributes.mutable_attributes.mode = Some(111);
        expected_attributes.mutable_attributes.gid = Some(222);
        assert_eq!(updated_attributes, expected_attributes);

        // Check that updating some of the attributes will not overwrite those that are not updated
        update_attributes_checked(
            &file,
            &fio::MutableNodeAttributes { uid: Some(333), gid: Some(444), ..Default::default() },
        )
        .await;
        let current_attributes = get_attributes_checked(
            &file,
            fio::NodeAttributesQuery::CREATION_TIME
                | fio::NodeAttributesQuery::MODIFICATION_TIME
                | fio::NodeAttributesQuery::MODE
                | fio::NodeAttributesQuery::UID
                | fio::NodeAttributesQuery::GID,
        )
        .await;
        expected_attributes.mutable_attributes.uid = Some(333);
        expected_attributes.mutable_attributes.gid = Some(444);
        assert_eq!(current_attributes, expected_attributes);

        // The contents of the file hasn't changed, so the flushed attributes should remain the same
        file.sync().await.unwrap().unwrap();
        let synced_attributes = get_attributes_checked(
            &file,
            fio::NodeAttributesQuery::CREATION_TIME
                | fio::NodeAttributesQuery::MODIFICATION_TIME
                | fio::NodeAttributesQuery::MODE
                | fio::NodeAttributesQuery::UID
                | fio::NodeAttributesQuery::GID,
        )
        .await;
        assert_eq!(synced_attributes, expected_attributes);

        close_file_checked(file).await;
        fixture.close().await;
    }

    // `update_attributes` flushes the attributes. We should check for race conditions where another
    // flush could occur at the same time.
    #[fuchsia::test(threads = 10)]
    async fn test_update_attributes_with_race() {
        let fixture = TestFixture::new_unencrypted().await;
        for i in 1..100 {
            let file_name = format!("file {}", i);
            let file1 = open_file_checked(
                fixture.root(),
                &file_name,
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let file2 = open_file_checked(
                fixture.root(),
                &file_name,
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            join!(
                fasync::Task::spawn(async move {
                    file1
                        .write("foo".as_bytes())
                        .await
                        .expect("FIDL call failed")
                        .map_err(zx::Status::err_from_raw)
                        .expect("write failed");
                    let write_modification_time =
                        get_attributes_checked(&file1, fio::NodeAttributesQuery::MODIFICATION_TIME)
                            .await
                            .mutable_attributes
                            .modification_time
                            .expect("get_attributes failed");

                    let now = Timestamp::now().as_nanos();
                    update_attributes_checked(
                        &file1,
                        &fio::MutableNodeAttributes {
                            modification_time: Some(now),
                            mode: Some(111),
                            gid: Some(222),
                            ..Default::default()
                        },
                    )
                    .await;
                    fasync::Timer::new(Duration::from_millis(10)).await;
                    let updated_attributes = get_attributes_checked(
                        &file1,
                        fio::NodeAttributesQuery::MODIFICATION_TIME
                            | fio::NodeAttributesQuery::MODE
                            | fio::NodeAttributesQuery::GID,
                    )
                    .await;

                    assert_ne!(
                        updated_attributes.mutable_attributes.modification_time.unwrap(),
                        write_modification_time
                    );
                    assert_eq!(updated_attributes.mutable_attributes.modification_time, Some(now));
                    assert_eq!(updated_attributes.mutable_attributes.mode, Some(111));
                    assert_eq!(updated_attributes.mutable_attributes.gid, Some(222));
                }),
                fasync::Task::spawn(async move {
                    for _ in 1..50 {
                        // Flush data
                        file2.sync().await.unwrap().unwrap();
                    }
                })
            );
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_write_timestamps() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file::write(&file, &[1, 2, 3, 4]).await.expect("write failed");
        // Remove `PENDING_ACCESS_TIME_UPDATE` from the query as no file access has been made.
        let write_attributes = get_attributes_checked(
            &file,
            fio::NodeAttributesQuery::all() - fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
        )
        .await;
        assert_eq!(
            write_attributes.mutable_attributes.modification_time,
            write_attributes.immutable_attributes.change_time
        );
        // Access time should not have been updated for a write
        assert!(
            write_attributes.mutable_attributes.access_time
                < write_attributes.mutable_attributes.modification_time
        );

        // Do something else that should not change mtime or ctime
        file.seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("FIDL call failed")
            .map_err(zx::ok)
            .expect("seek failed");
        file::read(&file).await.expect("read failed");
        let read_attributes = get_attributes_checked(&file, fio::NodeAttributesQuery::all()).await;
        assert!(
            read_attributes.mutable_attributes.access_time
                > write_attributes.mutable_attributes.access_time
        );
        assert_eq!(
            write_attributes.mutable_attributes.modification_time,
            read_attributes.mutable_attributes.modification_time,
        );
        assert_eq!(
            write_attributes.immutable_attributes.change_time,
            read_attributes.immutable_attributes.change_time,
        );

        // Syncing the file should have no affect on the timestamps
        file.sync().await.unwrap().unwrap();
        let sync_attributes = get_attributes_checked(
            &file,
            fio::NodeAttributesQuery::all() - fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
        )
        .await;
        assert_eq!(
            read_attributes.mutable_attributes.modification_time,
            sync_attributes.mutable_attributes.modification_time,
        );
        assert_eq!(
            read_attributes.immutable_attributes.change_time,
            sync_attributes.immutable_attributes.change_time,
        );
        assert_eq!(
            read_attributes.mutable_attributes.access_time,
            sync_attributes.mutable_attributes.access_time,
        );

        close_file_checked(file).await;
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_shrink_and_flush_updates_ctime() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let initial_file_size = PAGE_SIZE.get() as usize * 10;
        file::write(&file, vec![5u8; initial_file_size]).await.unwrap();
        file.sync().await.unwrap().map_err(zx::ok).unwrap();

        let (starting_mtime, starting_ctime) = {
            let attributes = get_attributes_checked(&file, fio::NodeAttributesQuery::all()).await;
            (
                attributes.mutable_attributes.modification_time,
                attributes.immutable_attributes.change_time,
            )
        };

        // Shrink the file size.
        file.resize(0).await.expect("FIDL call failed").expect("resize failed");
        // Check that the change in timestamps are preserved with flush.
        file.sync().await.unwrap().unwrap();

        let (synced_mtime, synced_ctime) = {
            let attributes = get_attributes_checked(&file, fio::NodeAttributesQuery::all()).await;
            (
                attributes.mutable_attributes.modification_time,
                attributes.immutable_attributes.change_time,
            )
        };

        assert!(starting_ctime < synced_ctime);
        assert!(starting_mtime < synced_mtime);
        assert_eq!(synced_ctime, synced_mtime);

        close_file_checked(file).await;
        fixture.close().await;
    }

    // Verifies that if a duplicate page-in races with sync, the second page-in cannot populate the
    // vmo with old data. While normally a duplicate page-in is dropped by the kernel, if the kernel
    // reclaims the page after flush it can accept the stale data served from the duplicate supply.
    // This test ensures that we do not mark the pages clean until all in-flight page-ins are gone
    // so that the page cannot be reclaimed.
    #[fuchsia::test(threads = 5)]
    async fn test_duplicate_page_in_race_with_sync() {
        // Populate 2 pages with 0x01 to be version 1 of the file.
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, _object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            std::thread::spawn(move || {
                let buf = vec![1u8; (PAGE_SIZE * 2) as usize];
                stream
                    .write_at(zx::StreamWriteOptions::empty(), 0, buf.as_slice())
                    .expect("Init file contents");
            })
            .join()
            .unwrap();
            proxy.sync().await.unwrap().expect("Sync");
        }

        // Remount to clear the cached vmo.
        let device = fixture.close().await;
        let fixture = TestFixture::open(
            device,
            TestFixtureOptions { encrypted: false, format: false, ..Default::default() },
        )
        .await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // ASCII approximation of a UML sequence diagram. Time flows down. The arrow destination
            // does not proceed until the action described by the arrow is triggered from the arrow
            // source.
            //
            // stream_write             test                 stream_read                 sync
            //   |                        |                        |                       |
            //   |   spawn stream_write  | |                       |                       |
            //  | | <|-------------------| |                       |                       |
            //  | |                      | |                       |                       |
            //  | |                      | |                       |                       |
            //  | |    first_read_tx     | |                       |                       |
            //  | |--------------------|>| |                       |                       |
            //  | |                      | |  spawn stream_read    |                       |
            //  | |                      | | -------------------|>| |                      |
            //  | |                      | |                      | |                      |
            //  | |                      | |                      | |                      |
            //  | |                      | |    second_read tx    | |                      |
            //  | |<|---------------------------------------------| |                      |
            //  | |                      | |                      | |                      |
            //  | |                      | |                      | |                      |
            //  | |                      | |                      | |                      |
            //  | |  await stream_write  | |                      | |                      |
            //    ---------------------|>| |                      | |                      |
            //   |                       | |                      | |      spawn sync      |
            //   |                       | | -------------------------------------------|>| |
            //   |                       | |                      | |                     | |
            //   |                       | |                      | |                     | |
            //   |                       | |     Data flushed     | |     to disk         | |
            //   |                       | |<|--------------------------------------------| |
            //   |                       | |                      | |                     | |
            //   |                       | |                      | |                     | |
            //   |        Sleep for sync to block on epoch        | |                     | |
            //   |                       | |                      | |                     | |
            //   |                       | |                      | |                     | |
            //   |                 Bytes still dirty              | |                     | |
            //   |                       | |                      | |                     | |
            //   |                       | |                      | |                     | |
            //   |                       | |  syncing_blocked_tx  | |                     | |
            //   |                       | |--------------------|>| |                     | |
            //   |                       | |                      | |    epoch dropped    | |
            //   |                       | |                      | |-------------------|>| |
            //   |                       | |   await stream_read  | |                     | |
            //   |                       | |<|--------------------| |                     | |
            //   |                       | |                       |                      | |
            //   |                       | |     await sync        |                      | |
            //   |                       | |<|--------------------------------------------| |
            //   |                       | |                       |                       |
            //   |                        |                        |                       |

            // The actions_* channel is used to pipe a sender and receiver to each read that we want
            // to block. Those function as oneshot signals to send out that they've reached the
            // critical point and wait to before moving on from it. They need to be sync in places
            // so we can't use the fasync oneshot.
            let (actions_tx, actions_rx) =
                mpsc::channel::<(mpsc::Sender<()>, mpsc::Receiver<()>)>();
            let actions_rx = Mutex::new(actions_rx);
            let guard = CALLBACK_AFTER_READ_UNCACHED.set(move || {
                let (tx, rx) = actions_rx.lock().recv().unwrap();
                tx.send(()).expect("Sending one-shot");
                rx.recv().expect("Awaiting one-shot");
            });
            // These should all be one-shots but they need to be sync, and sync
            // one-shots are still in nightly.
            let (first_read_tx, first_read_rx) = mpsc::channel::<()>();
            let (second_read_tx, second_read_rx) = mpsc::channel::<()>();
            let (syncing_blocked_tx, syncing_blocked_rx) = mpsc::channel::<()>();
            actions_tx.send((first_read_tx, second_read_rx)).expect("Sending actions");
            actions_tx.send((second_read_tx, syncing_blocked_rx)).expect("Sending actions");

            // Now write version 2 directly to stream. This will trigger a read first, but we don't
            // want the read to complete until after the next parallel read is part-way done.
            let stream_clone = stream.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
            let write_thread = fasync::unblock(move || {
                stream_clone
                    .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[2u8; 10])
                    .expect("stream_write");
            });

            // Wait for it to get blocked.
            first_read_rx.recv().unwrap();

            // Trigger a read request at a different unpopulated page in parallel. This will spawn
            // an async page_in request on the executor holding an EpochGuard and unblock the first
            // stream request.
            let stream_clone = stream.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
            let read_thread = fasync::unblock(move || {
                stream_clone.read_to_vec(zx::StreamReadOptions::empty(), 8).expect("stream_read");
            });

            write_thread.await;

            // Start sync, and see that it is blocked at a point where the data is updated but the
            // pages aren't marked clean yet. It is critical that the page in barrier is there.
            let (syncing_done_tx, syncing_done_rx) = mpsc::channel::<()>();
            let sync_task = fasync::Task::spawn(async move {
                proxy.sync().await.unwrap().expect("Sync");
                syncing_done_tx.send(()).unwrap();
            });
            // Drop the guard so that these reads don't get blocked.
            std::mem::drop(guard);
            while object
                .handle()
                .read_uncached(PAGE_SIZE.get()..(PAGE_SIZE * 2))
                .await
                .unwrap()
                .to_vec()[0]
                == 1u8
            {
                fasync::Timer::new(std::time::Duration::from_millis(50)).await;
            }
            // Wait at least this long after data is confirmed written out and check that it still
            // hasn't finished so that we know it is well and truly blocked.
            fasync::Timer::new(std::time::Duration::from_millis(50)).await;
            assert_matches!(syncing_done_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
            let page_size = PAGE_SIZE.get();
            assert!(
                object
                    .handle()
                    .collect_modified_ranges()
                    .expect("Collecting ranges")
                    .iter()
                    .any(|r| r.range().contains(&page_size))
            );

            // Release the second read and everything finishes.
            syncing_blocked_tx.send(()).unwrap();
            read_thread.await;
            sync_task.await;

            assert_eq!(
                stream
                    .read_at_to_vec(zx::StreamReadOptions::empty(), PAGE_SIZE.get(), 10)
                    .expect("Reading"),
                vec![2u8; 10]
            );
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_vmo_write_beyond_content_size_doesnt_break_flush() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        // Write out three pages initially.
        let file_size = PAGE_SIZE * 3;
        fuchsia_fs::file::write(&file, &vec![1u8; file_size as usize]).await.unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        // Get the backing memory for the file. Confirm the length of the vmo and the reported
        // stream size.
        let vmo = file
            .get_backing_memory(fio::VmoFlags::READ | fio::VmoFlags::WRITE)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        assert_eq!(vmo.get_stream_size().unwrap(), file_size);

        // Resize the file down to one page. Confirm the stream size is updated, but the vmo size
        // stays the same.
        file.resize(PAGE_SIZE.get()).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        assert_eq!(vmo.get_stream_size().unwrap(), PAGE_SIZE);

        // Write some data to the vmo, beyond the current stream size. This does _not_ update the
        // stream size, but it does make pages dirty beyond the end of the file.
        unblock(move || {
            vmo.write(&[1, 2, 3, 4], PAGE_SIZE * 2).unwrap();
            // Writing this data to the vmo shouldn't update the stream size.
            assert_eq!(vmo.get_stream_size().unwrap(), PAGE_SIZE);
        })
        .await;

        fixture.close().await;
    }

    async fn open_file_proxy_object_and_stream(
        fixture: &TestFixture,
    ) -> (fio::FileProxy, Arc<FxFile>, zx::Stream) {
        let proxy = open_file_checked(
            fixture.root(),
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let id = proxy
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await
            .unwrap()
            .expect("Get attr")
            .1
            .id
            .expect("Missing id");
        let object = fixture
            .volume()
            .volume()
            .cache()
            .get(id)
            .expect("Node should be live")
            .into_any()
            .downcast::<FxFile>()
            .unwrap();

        let stream = proxy.describe().await.unwrap().stream.unwrap();
        (proxy, object, stream)
    }

    #[fuchsia::test(threads = 3)]
    async fn test_race_mark_dirty_before_collection() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[1u8]).expect("First dirty page");
            let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                stream
                    .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[2u8])
                    .expect("Second dirty page");
            });

            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_race_eof_flush_corruption() {
        let fixture = TestFixture::new_unencrypted().await;
        let concurrent_write_start;

        {
            let (proxy, _, stream) = open_file_proxy_object_and_stream(&fixture).await;

            let initial_pages = 125;
            let initial_size = initial_pages * PAGE_SIZE;
            let buf = vec![0xaa; initial_size as usize];
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &buf).expect("Initial write");
            proxy.sync().await.unwrap().expect("Initial sync");

            // Write 0-10KB (Overwrite)
            let overwrite_size = 10 * 1024;
            let overwrite_buf = vec![0xbb; overwrite_size];
            stream
                .write_at(zx::StreamWriteOptions::empty(), 0, &overwrite_buf)
                .expect("Overwrite write");

            // Write 512000 to 613376 (extends file, Cow)
            let extension_end = 599 * 1024;
            let extension_size = extension_end - initial_size;
            let extension_buf = vec![0xcc; extension_size as usize];
            stream
                .write_at(zx::StreamWriteOptions::empty(), initial_size, &extension_buf)
                .expect("Extension write");

            concurrent_write_start = extension_end;
            let concurrent_write_end = 700 * 1024;
            let concurrent_write_size = concurrent_write_end - concurrent_write_start;
            let concurrent_write_buf = vec![0xdd; concurrent_write_size as usize];

            let stream_clone =
                stream.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Duplicating stream");
            let _guard = CALLBACK_AFTER_SIZE_CAPTURE.set(move || {
                stream_clone
                    .write_at(
                        zx::StreamWriteOptions::empty(),
                        concurrent_write_start,
                        &concurrent_write_buf,
                    )
                    .expect("Concurrent write");
            });

            proxy.sync().await.unwrap().expect("Syncing");
        }

        let device = fixture.close().await;

        // Remount
        let fixture = TestFixture::open(
            device,
            TestFixtureOptions { encrypted: false, format: false, ..Default::default() },
        )
        .await;

        {
            let (_proxy, _, stream) = open_file_proxy_object_and_stream(&fixture).await;
            let read_buf = stream
                .read_at_to_vec(zx::StreamReadOptions::empty(), concurrent_write_start, 100)
                .expect("Reading back");
            assert_eq!(read_buf[0], 0xdd);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_race_mark_dirty_before_after_collection() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;
            let stream2 =
                stream.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Duplicating stream");

            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[1u8]).expect("First dirty page");
            stream
                .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[1u8])
                .expect("First dirty page");

            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 2);
            {
                let object_clone = object.clone();
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.total(), 0);
                    // This page will be found in the page collection.
                    stream
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 2, &[2u8])
                        .expect("Second dirty page");
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.total(), 1);
                });

                let object_clone = object.clone();
                let _guard2 = CALLBACK_AFTER_RANGE_COLLECTION.set(move || {
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.total(), 1);
                    // This page will be missed.
                    stream2
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 3, &[3u8])
                        .expect("Second dirty page");
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.total(), 2);
                });

                proxy.sync().await.unwrap().expect("Syncing");
            }
            // The missed page will remain and needs to be flushed.
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 1);

            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_race_mark_dirty_unreserved_before_collection() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // Pre-allocate 4 pages to make them unreserved.
            proxy
                .allocate(0, PAGE_SIZE * 4, fio::AllocateMode::empty())
                .await
                .unwrap()
                .expect("Allocate failed");

            // Now write to the first two pages to make them dirty and unreserved.
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[2u8]).unwrap();
            stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[2u8]).unwrap();

            assert_eq!(object.handle().inner.lock().dirty_pages.unreserved, 2);

            let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                // Write to the next two pages during the race. They should also be unreserved.
                stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 2, &[2u8]).unwrap();
                stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 3, &[2u8]).unwrap();
            });

            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_race_mark_dirty_unreserved_before_after_collection() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;
            let stream2 =
                stream.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Duplicating stream");

            // Pre-allocate 4 pages to make them unreserved.
            proxy
                .allocate(0, PAGE_SIZE * 4, fio::AllocateMode::empty())
                .await
                .unwrap()
                .expect("Allocate failed");

            // Now write to the first two pages to make them dirty and unreserved.
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[2u8]).unwrap();
            stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[2u8]).unwrap();

            assert_eq!(object.handle().inner.lock().dirty_pages.unreserved, 2);

            {
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    // This page will be found in the page collection.
                    stream
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 2, &[2u8])
                        .unwrap();
                });

                let _guard2 = CALLBACK_AFTER_RANGE_COLLECTION.set(move || {
                    // This page will be missed.
                    stream2
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 3, &[2u8])
                        .unwrap();
                });

                proxy.sync().await.unwrap().expect("Syncing");
            }
            // The missed page will remain and needs to be flushed.
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 1);

            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_shift_overwrite_to_cow() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // Pre-allocate 2 pages to make them unreserved.
            proxy
                .allocate(0, PAGE_SIZE * 2, fio::AllocateMode::empty())
                .await
                .unwrap()
                .expect("Allocate failed");

            // Now write to make them dirty (unreserved).
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[1u8]).unwrap();
            stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[1u8]).unwrap();

            proxy.resize(0).await.unwrap().expect("Truncating");

            assert_eq!(object.handle().inner.lock().dirty_pages.unreserved, 2);
            assert_eq!(object.handle().inner.lock().dirty_pages.reserved, 0);

            {
                let object_clone = object.clone();
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.total(), 0);
                    // Write the 2 pages back COW.
                    stream.write_at(zx::StreamWriteOptions::empty(), 0, &[2u8]).unwrap();
                    stream
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[2u8])
                        .unwrap();
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.reserved, 2);
                });

                proxy.sync().await.unwrap().expect("Syncing");
            }
            // The flush clears the reserved pages, but can't know if the unreserved pages can be
            // cleaned up since they could have just as well been dirtied while the range was being
            // collected, so they get put back.
            assert_eq!(object.handle().inner.lock().dirty_pages.unreserved, 2);
            assert_eq!(object.handle().inner.lock().dirty_pages.reserved, 0);

            // Another flush with no race will clean it up, even though no pages get flushed.
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    // If the file had several dirty pages and then was truncated to before those dirty pages
    // then we'll still have space reserved that is no longer needed and should be released as
    // part of this flush.
    //
    // If `reservation` and `dirty_pages` were pulled out of `inner` after calling
    // `query_dirty_ranges` then we wouldn't be able to tell the difference between pages there
    // dirtied between those 2 operations and dirty pages that were made irrelevant by the
    // truncate.
    #[fuchsia::test(threads = 3)]
    async fn test_truncate_shorter_race() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // Write 3 pages to make them dirty.
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[1u8]).unwrap();
            stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[2u8]).unwrap();
            stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 2, &[3u8]).unwrap();

            // File gets truncated, losing 2 of them.
            proxy.resize(PAGE_SIZE.get()).await.unwrap().expect("Truncating");

            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 3);

            {
                let object_clone = object.clone();
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.total(), 0);
                    // Write 4 pages in the race. This makes the number of pages that get flushed
                    // equal what gets flushed, but with leftover pages still dirty.
                    stream
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[2u8])
                        .unwrap();
                    stream
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 2, &[3u8])
                        .unwrap();
                    stream
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 3, &[4u8])
                        .unwrap();
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.total(), 3);
                });

                proxy.sync().await.unwrap().expect("Syncing");
            }

            // The 2 lost pages should be left over.
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 2);

            // Another sync should clean them up.
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_write_past_end_not_flushed() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // Write 3 pages to make them dirty.
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[1u8]).unwrap();
            stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[1u8]).unwrap();
            stream.write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE * 2, &[1u8]).unwrap();

            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);

            // Get a public writeable copy of the vmo.
            let vmo = proxy
                .get_backing_memory(fio::VmoFlags::READ | fio::VmoFlags::WRITE)
                .await
                .unwrap()
                .expect("Get backing memory");

            // Truncate to 2 pages. Page 2 is now past end.
            proxy.resize(PAGE_SIZE * 2).await.unwrap().expect("Truncating");

            // Now dirty all three from the vmo.
            vmo.write(&[2u8], 0).expect("Writing vmo page 0");
            vmo.write(&[2u8], PAGE_SIZE.get()).expect("Writing vmo page 1");
            vmo.write(&[2u8], PAGE_SIZE * 2).expect("Writing vmo page 2");

            {
                let object_clone = object.clone();
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    // All the dirty pages were taken.
                    assert_eq!(object_clone.handle().inner.lock().dirty_pages.total(), 0);
                });

                // Sync. This should flush pages 0 and 1, but NOT page 2.
                proxy.sync().await.unwrap().expect("Syncing");
            }

            // Page 2 should be left over. It must have been put back.
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 1);

            // Now we need a "last chance" flush to clean it up.
            object.handle().flush(FlushType::LastChance).await.unwrap();
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_partial_flush_failure() {
        let fail = Arc::new(AtomicU64::new(u64::MAX));
        let fail_clone = fail.clone();
        let (mut hooks, fs_hooks) = fxfs::hooks::Hooks::new();
        hooks.set_pre_commit(move |_| {
            if fail_clone.fetch_sub(1, Ordering::Relaxed) == 0 {
                Err(FxfsError::Unavailable.into())
            } else {
                Ok(())
            }
        });
        let fixture = TestFixture::open(
            DeviceHolder::new(FakeDevice::new(16384, 512)),
            TestFixtureOptions { encrypted: false, hooks: Some(fs_hooks), ..Default::default() },
        )
        .await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // Start with 4 pages. 2 reserved, 2 unreserved. Dirty all of them.
            proxy.resize(PAGE_SIZE * 4).await.unwrap().expect("Truncating");
            proxy
                .allocate(PAGE_SIZE * 2, PAGE_SIZE * 4, fio::AllocateMode::empty())
                .await
                .unwrap()
                .expect("Allocate failed");
            for i in 0..4 {
                stream
                    .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[1u8])
                    .expect("Dirtying pages");
            }

            // Reserved and unreserved are flushed as 2 batches. Make the second batch fail.
            assert_eq!(object.handle().inner.lock().dirty_pages.unreserved, 2);
            assert_eq!(object.handle().inner.lock().dirty_pages.reserved, 2);
            {
                // One of the batches will succeed, the other will fail.
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    fail.store(1, Ordering::Relaxed);
                });
                proxy.sync().await.unwrap().expect_err("Partial flush success");
            }
            // Last two pages get cleaned up with another flush.
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 2);
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }

        fixture.close().await;
    }

    // Same as above, but does a last chance flush that gives up with possible data loss.
    #[fuchsia::test(threads = 3)]
    async fn test_partial_flush_failure_last_chance() {
        let fail = Arc::new(AtomicU64::new(u64::MAX));
        let fail_clone = fail.clone();
        let (mut hooks, fs_hooks) = fxfs::hooks::Hooks::new();
        hooks.set_pre_commit(move |_| {
            if fail_clone.fetch_sub(1, Ordering::Relaxed) == 0 {
                Err(FxfsError::Unavailable.into())
            } else {
                Ok(())
            }
        });
        let fixture = TestFixture::open(
            DeviceHolder::new(FakeDevice::new(16384, 512)),
            TestFixtureOptions { encrypted: false, hooks: Some(fs_hooks), ..Default::default() },
        )
        .await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // Start with 4 pages. 2 reserved, 2 unreserved. Dirty all of them.
            proxy.resize(PAGE_SIZE * 4).await.unwrap().expect("Truncating");
            proxy
                .allocate(PAGE_SIZE * 2, PAGE_SIZE * 4, fio::AllocateMode::empty())
                .await
                .unwrap()
                .expect("Allocate failed");
            for i in 0..4 {
                stream
                    .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[1u8])
                    .expect("Dirtying pages");
            }

            // Reserved and unreserved are flushed as 2 batches. Make the second batch fail.
            assert_eq!(object.handle().inner.lock().dirty_pages.unreserved, 2);
            assert_eq!(object.handle().inner.lock().dirty_pages.reserved, 2);
            {
                // One of the batches will succeed, the other will fail.
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    fail.store(1, Ordering::Relaxed);
                });
                object
                    .handle()
                    .flush(FlushType::LastChance)
                    .await
                    .expect_err("Partial flush success");
            }
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_race_mark_dirty_with_truncation() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // First write the file and sync it.
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[1u8]).unwrap();
            proxy.sync().await.unwrap().expect("Syncing");

            // Now truncate the file back to zero, and dirty the page while it syncs it.
            proxy.resize(0).await.unwrap().expect("Truncating");
            {
                let stream2 =
                    stream.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Duplicating stream");
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    stream2.write_at(zx::StreamWriteOptions::empty(), 0, &[2u8]).unwrap();
                });
                proxy.sync().await.unwrap().expect("Syncing");
            }

            // One sync without races to restore reliable state.
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);

            // Do it again, but put the race in after the ranges are collected.
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[3u8]).unwrap();
            proxy.sync().await.unwrap().expect("Syncing");
            proxy.resize(0).await.unwrap().expect("Truncating");
            {
                let _guard = CALLBACK_AFTER_RANGE_COLLECTION.set(move || {
                    stream.write_at(zx::StreamWriteOptions::empty(), 0, &[4u8]).unwrap();
                });
                proxy.sync().await.unwrap().expect("Syncing");
            }

            // One sync without races to restore reliable state.
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_race_mark_dirty_with_background_flush_cow() {
        race_mark_dirty_with_background_flush(false).await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_race_mark_dirty_with_background_flush_overwrite() {
        race_mark_dirty_with_background_flush(true).await;
    }

    async fn race_mark_dirty_with_background_flush(allocate_range: bool) {
        let fixture = TestFixture::open(
            DeviceHolder::new(FakeDevice::new(65536, 512)),
            TestFixtureOptions::default(),
        )
        .await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;
            let background_pages_threshold = BACKGROUND_FLUSH_THRESHOLD / PAGE_SIZE;

            if allocate_range {
                proxy
                    .allocate(
                        0,
                        BACKGROUND_FLUSH_THRESHOLD + PAGE_SIZE * 2,
                        fio::AllocateMode::empty(),
                    )
                    .await
                    .unwrap()
                    .expect("Allocate");
                proxy.sync().await.unwrap().expect("Sync after allocate");
            }

            for i in 0..background_pages_threshold {
                stream
                    .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[1u8])
                    .expect("Dirty page");
            }

            // No background flush yet.
            assert_eq!(
                object.handle().inner.lock().dirty_pages.total(),
                background_pages_threshold
            );

            let flush_counter = Arc::new(AtomicU64::new(0));
            let flush_counter_clone = flush_counter.clone();
            let stream_dup = stream.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
            let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                // Dirty one more page.
                if flush_counter_clone.fetch_add(1, Ordering::Relaxed) == 0 {
                    stream_dup
                        .write_at(
                            zx::StreamWriteOptions::empty(),
                            BACKGROUND_FLUSH_THRESHOLD + PAGE_SIZE,
                            &[1u8],
                        )
                        .expect("Dirty one page race");
                }
            });
            // This should trigger the background flush.
            stream
                .write_at(zx::StreamWriteOptions::empty(), BACKGROUND_FLUSH_THRESHOLD, &[1u8])
                .expect("Dirty page");

            let mut timeout = Duration::from_secs(10);
            while flush_counter.load(Ordering::Relaxed) == 0 {
                let increment = Duration::from_millis(5);
                fasync::Timer::new(increment).await;
                assert!(timeout > increment, "Timed out awaiting background flush");
                timeout -= increment;
            }
            // After the flush has started, take a truncate lock on the file, to see that the flush
            // has finished.
            let _ = fixture
                .fs()
                .truncate_guard(
                    fixture.volume().volume().store().store_object_id(),
                    object.object_id(),
                )
                .await;

            // The two pages should be left dirty.
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 2);

            // See that they can be cleaned up normally.
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }

        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_background_flush_with_allocated_shift_to_cow() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;
            let batch_pages = FLUSH_BATCH_SIZE / PAGE_SIZE;

            // Allocate 2 pages, dirty them, then truncate away the second one.
            proxy
                .allocate(0, PAGE_SIZE * 2, fio::AllocateMode::empty())
                .await
                .unwrap()
                .expect("Allocate");
            proxy.sync().await.unwrap().expect("Sync after allocate");
            for i in 0..2 {
                stream
                    .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[1u8])
                    .expect("Dirty page");
            }
            proxy.resize(PAGE_SIZE.get()).await.unwrap().expect("Truncating");

            // Write one full batch of COW starting from the second page.
            for i in 1..(batch_pages + 1) {
                stream
                    .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[1u8])
                    .expect("Dirty page");
            }

            // Trigger a background flush.
            object.handle().flush(FlushType::Background).await.expect("Flushing");
            // The one allocated page should have been ignored. Not an entire batch.
            assert_eq!(object.handle().inner.lock().dirty_pages.unreserved, 1);

            // Dirty one more page to make the file need a flush then do a full flush. Everything
            // should be cleared.
            stream
                .write_at(zx::StreamWriteOptions::empty(), (batch_pages + 1) * PAGE_SIZE, &[1u8])
                .expect("Dirty page");
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 3)]
    async fn test_background_flush_with_cow_shift_to_allocated() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;
            let batch_pages = FLUSH_BATCH_SIZE / PAGE_SIZE;

            // Allocate 1 page.
            proxy
                .allocate(0, PAGE_SIZE.get(), fio::AllocateMode::empty())
                .await
                .unwrap()
                .expect("Allocate");

            {
                // Allocate a batch where it gets dirtied as COW between the sync that allocate does
                // and the actual allocation. Dirty one page as cow afterwards as well.
                let stream2 = stream.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                let _guard = CALLBACK_AFTER_RANGE_COLLECTION.set(move || {
                    for i in 0..(batch_pages + 2) {
                        stream2
                            .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[1u8])
                            .expect("Dirty page");
                    }
                });

                proxy
                    .allocate(PAGE_SIZE.get(), batch_pages * PAGE_SIZE, fio::AllocateMode::empty())
                    .await
                    .unwrap()
                    .expect("Allocate");
            }
            // Trigger a background flush with a race to force taking dirty pages again.
            {
                let stream2 = stream.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    stream2
                        .write_at(
                            zx::StreamWriteOptions::empty(),
                            (batch_pages + 2) * PAGE_SIZE,
                            &[1u8],
                        )
                        .expect("Dirty page");
                });

                object.handle().flush(FlushType::Background).await.expect("Flushing");
            }

            // Dirty one more page to make the file need a flush then do a full flush. Everything
            // should be cleared.
            stream
                .write_at(zx::StreamWriteOptions::empty(), (batch_pages + 3) * PAGE_SIZE, &[1u8])
                .expect("Dirty page");
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    // This triggers putting back more unreserved pages than were ever marked dirty. This would have
    // cause an underflow if the clean pages calculation is not handled properly.
    #[fuchsia::test(threads = 3)]
    async fn test_background_flush_with_cow_shift_to_allocated_underflow() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;
            let batch_pages = FLUSH_BATCH_SIZE / PAGE_SIZE;

            {
                // Allocate a batch where it gets dirtied as COW between the sync that allocate does
                // and the actual allocation. Dirty one page as cow afterwards as well.
                let stream2 = stream.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                let _guard = CALLBACK_AFTER_RANGE_COLLECTION.set(move || {
                    for i in 0..(batch_pages + 2) {
                        stream2
                            .write_at(zx::StreamWriteOptions::empty(), i * PAGE_SIZE, &[1u8])
                            .expect("Dirty page");
                    }
                });

                proxy
                    .allocate(0, (batch_pages + 2) * PAGE_SIZE, fio::AllocateMode::empty())
                    .await
                    .unwrap()
                    .expect("Allocate");
            }
            // Trigger a background flush with a race to force taking dirty pages again.
            {
                let stream2 = stream.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    stream2
                        .write_at(
                            zx::StreamWriteOptions::empty(),
                            (batch_pages + 2) * PAGE_SIZE,
                            &[1u8],
                        )
                        .expect("Dirty page");
                });

                object.handle().flush(FlushType::Background).await.expect("Flushing");
            }

            // Dirty one more page to make the file need a flush then do a full flush. Everything
            // should be cleared.
            stream
                .write_at(zx::StreamWriteOptions::empty(), (batch_pages + 3) * PAGE_SIZE, &[1u8])
                .expect("Dirty page");
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    // Ensure that when background flush returns early it puts back the correct number of dirty
    // pages.
    #[fuchsia::test(threads = 3)]
    async fn test_background_flush_underflow_race() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let (proxy, object, stream) = open_file_proxy_object_and_stream(&fixture).await;

            // Mark 1 page dirty via normal write. This will get needs_flush() returning true.
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &[1u8]).expect("Dirty page");

            {
                let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                    // This should get an extra page in the range collection. So now
                    // dirty_pages_not_to_flush will be 2 while marked_dirty_pages is 1.
                    stream
                        .write_at(zx::StreamWriteOptions::empty(), PAGE_SIZE.get(), &[1u8])
                        .expect("Dirty page in a race");
                });

                object.handle().flush(FlushType::Background).await.expect("Flush failed");
            }
            proxy.sync().await.unwrap().expect("Syncing");
            assert_eq!(object.handle().inner.lock().dirty_pages.total(), 0);
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_shrink_with_dirty_pages() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let root = fixture.root();
            let file = open_file_checked(
                &root,
                FILE_NAME,
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let file_obj = {
                let id = file
                    .get_attributes(fio::NodeAttributesQuery::ID)
                    .await
                    .unwrap()
                    .expect("Get attr")
                    .1
                    .id
                    .expect("Missing id");
                fixture
                    .volume()
                    .volume()
                    .cache()
                    .get(id)
                    .expect("Node should be live")
                    .into_any()
                    .downcast::<FxFile>()
                    .unwrap()
            };

            assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 0);
            file.resize(PAGE_SIZE * 2).await.unwrap().expect("Grow file");
            file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
            file.write_at(&[1, 2, 3, 4], PAGE_SIZE.get()).await.unwrap().expect("Writing");
            assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 1);
            file.resize(PAGE_SIZE.get()).await.unwrap().expect("Shrink file");
            file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
            assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 0);
            file.write_at(&[1, 2, 3, 4], PAGE_SIZE.get()).await.unwrap().expect("Writing");
            assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 1);
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_shrink_with_allocated_dirty_pages() {
        let fixture = TestFixture::new_unencrypted().await;
        {
            let root = fixture.root();
            let file = open_file_checked(
                &root,
                FILE_NAME,
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            let file_obj = {
                let id = file
                    .get_attributes(fio::NodeAttributesQuery::ID)
                    .await
                    .unwrap()
                    .expect("Get attr")
                    .1
                    .id
                    .expect("Missing id");
                fixture
                    .volume()
                    .volume()
                    .cache()
                    .get(id)
                    .expect("Node should be live")
                    .into_any()
                    .downcast::<FxFile>()
                    .unwrap()
            };

            assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 0);
            file.allocate(0, PAGE_SIZE * 2, fio::AllocateMode::empty())
                .await
                .unwrap()
                .expect("Allocate file");
            file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
            file.write_at(&[1, 2, 3, 4], PAGE_SIZE.get()).await.unwrap().expect("Writing");
            assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 1);
            file.resize(PAGE_SIZE.get()).await.unwrap().expect("Shrink file");
            file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
            assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 0);
            file.write_at(&[1, 2, 3, 4], PAGE_SIZE.get()).await.unwrap().expect("Writing");
            assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 1);
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_allocate() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file::write(&file, &vec![1, 2, 3, 4]).await.unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.allocate(0, PAGE_SIZE.get(), fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let data = file.read_at(4, 0).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        assert_eq!(data, vec![1, 2, 3, 4]);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_allocate_empty() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        assert_eq!(
            file.get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
                .await
                .unwrap()
                .unwrap()
                .1
                .content_size
                .unwrap(),
            0,
        );

        file.allocate(0, PAGE_SIZE.get(), fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let data = file
            .read_at(PAGE_SIZE.get(), 0)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        assert_eq!(data, vec![0; PAGE_SIZE.get() as usize]);

        assert_eq!(
            file.get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
                .await
                .unwrap()
                .unwrap()
                .1
                .content_size
                .unwrap(),
            PAGE_SIZE,
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_allocate_write() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.allocate(0, PAGE_SIZE.get(), fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        file::write(&file, &vec![1, 2, 3, 4]).await.unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        let data = file.read_at(4, 0).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        assert_eq!(data, vec![1, 2, 3, 4]);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_allocate_write_mixed() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.allocate(PAGE_SIZE.get(), PAGE_SIZE.get(), fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let write_data = (0..20).cycle().take((PAGE_SIZE * 2) as usize).collect::<Vec<_>>();
        assert_eq!(
            file.write_at(&write_data, 2048)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE * 2
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        let data = file
            .read_at(PAGE_SIZE * 2, 2048)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        assert_eq!(data, write_data);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_allocate_write_disk_full() {
        let fixture = TestFixture::new_unencrypted().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.allocate(0, PAGE_SIZE.get(), fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let write_data = (0..20).cycle().take(PAGE_SIZE.get() as usize).collect::<Vec<_>>();
        // Fill up the disk with data.
        loop {
            match file.write(&write_data).await.unwrap().map_err(zx::Status::err_from_raw) {
                Ok(len) => assert_eq!(len, PAGE_SIZE),
                Err(status) => {
                    assert_eq!(status, zx::Status::NO_SPACE);
                    break;
                }
            }
            file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        }

        // Writing outside the allocated range fails (because not overwrite mode.)
        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::NO_SPACE)
        );

        for _ in 0..100 {
            // Writing inside the allocated range succeeds indefinitely (because overwrite mode).
            assert_eq!(
                file.write_at(&write_data, 0)
                    .await
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
                    .unwrap(),
                PAGE_SIZE
            );
            file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        }

        // Note that it is possible now that writing outside the range may work again because
        // the overwrite transactions committed above as part of the fallocate range above will
        // consume journal space and this space may lead to a compaction that frees the prefix of
        // the journal, creating up to around 128kb of available space.

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_allocate_write_disk_full_multi_file() {
        let write_data = (0..20).cycle().take(PAGE_SIZE.get() as usize).collect::<Vec<_>>();

        let device = {
            let fixture = TestFixture::new_unencrypted().await;
            let root = fixture.root();

            {
                let file = open_file_checked(
                    &root,
                    FILE_NAME,
                    fio::Flags::FLAG_MAYBE_CREATE
                        | fio::PERM_READABLE
                        | fio::PERM_WRITABLE
                        | fio::Flags::PROTOCOL_FILE,
                    &Default::default(),
                )
                .await;
                file.allocate(0, PAGE_SIZE * 4, fio::AllocateMode::empty())
                    .await
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
                    .unwrap();
                file.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
            }

            fixture.close().await
        };

        let fixture = TestFixture::open(
            device,
            TestFixtureOptions { encrypted: false, format: false, ..Default::default() },
        )
        .await;
        let root = fixture.root();

        let filler_file = open_file_checked(
            &root,
            "filler",
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        loop {
            match filler_file.write(&write_data).await.unwrap().map_err(zx::Status::err_from_raw) {
                Ok(len) => assert_eq!(len, PAGE_SIZE),
                Err(status) => {
                    assert_eq!(status, zx::Status::NO_SPACE);
                    break;
                }
            }
        }

        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        // Writing outside the allocated range fails.
        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE * 4)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::NO_SPACE)
        );

        for _ in 0..100 {
            // Writing inside the allocated range succeeds indefinitely.
            assert_eq!(
                file.write_at(&write_data, 0)
                    .await
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
                    .unwrap(),
                PAGE_SIZE
            );
            assert_eq!(
                file.write_at(&write_data, PAGE_SIZE.get())
                    .await
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
                    .unwrap(),
                PAGE_SIZE
            );
            assert_eq!(
                file.write_at(&write_data, PAGE_SIZE * 2)
                    .await
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
                    .unwrap(),
                PAGE_SIZE
            );
            assert_eq!(
                file.write_at(&write_data, PAGE_SIZE * 3)
                    .await
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
                    .unwrap(),
                PAGE_SIZE
            );
            file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_file_allocate_write_restart() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.allocate(0, PAGE_SIZE * 4, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let write_data = (0..20).cycle().take(PAGE_SIZE.get() as usize).collect::<Vec<_>>();
        let write_data_alternate =
            (0..15).cycle().take(PAGE_SIZE.get() as usize).collect::<Vec<_>>();
        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE * 2)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        // Sync will make a transaction with whatever we have written. Make sure that there are
        // multiple transactions hitting the same blocks, to try and trip up the replay.
        assert_eq!(
            file.write_at(&write_data_alternate, 0)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        assert_eq!(
            file.write_at(&write_data_alternate, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        assert_eq!(
            file.read_at(PAGE_SIZE.get(), 0)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data_alternate,
        );
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data_alternate,
        );
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE * 2)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data,
        );
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE * 3)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            vec![0; PAGE_SIZE.get() as usize],
        );

        let device = fixture.close().await;

        let fixture = TestFixture::new_with_device(device).await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        assert_eq!(
            file.read_at(PAGE_SIZE.get(), 0)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data_alternate,
        );
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data_alternate,
        );
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE * 2)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data,
        );
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE * 3)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            vec![0; PAGE_SIZE.get() as usize],
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_truncate_allocated_file() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        let file_obj = {
            let id = file
                .get_attributes(fio::NodeAttributesQuery::ID)
                .await
                .unwrap()
                .expect("Get attr")
                .1
                .id
                .expect("Missing id");
            fixture
                .volume()
                .volume()
                .cache()
                .get(id)
                .expect("Node should be live")
                .into_any()
                .downcast::<FxFile>()
                .unwrap()
        };
        assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 0);

        file.allocate(0, PAGE_SIZE * 2, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let write_data = (0..20).cycle().take(PAGE_SIZE.get() as usize).collect::<Vec<_>>();
        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        file.resize(PAGE_SIZE.get()).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        assert_eq!(file_obj.handle().inner.lock().dirty_pages.total(), 0);

        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        // Should be a COW dirty range now.
        assert_eq!(file_obj.handle().inner.lock().dirty_pages.reserved, 1);
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data,
        );
        std::mem::drop(file_obj);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_allocate_unaligned() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.allocate(20, 100, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();

        let (_, attrs) = file
            .get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        assert_eq!(attrs.content_size, Some(120));

        let write_data = (0..20).cycle().take(PAGE_SIZE.get() as usize).collect::<Vec<_>>();
        assert_eq!(
            file.write_at(&write_data, 0).await.unwrap().map_err(zx::Status::err_from_raw).unwrap(),
            PAGE_SIZE
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        let (_, attrs) = file
            .get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        assert_eq!(attrs.content_size, Some(PAGE_SIZE.get()));

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_allocate_unaligned_prewritten_data() {
        // Test to confirm that 1. unaligned allocate works on existing extents, and 2. if the size
        // is updated, any data between the old and new size is properly zeroed.
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let write_data = (0..20).cycle().take(100).collect::<Vec<_>>();
        assert_eq!(
            file.write_at(&write_data, 0).await.unwrap().map_err(zx::Status::err_from_raw).unwrap(),
            100,
        );
        assert_eq!(
            file.write_at(&write_data, 100)
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            100,
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.resize(100).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        let (_, attrs) = file
            .get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        assert_eq!(attrs.content_size, Some(100));

        file.allocate(0, 150, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let (_, attrs) = file
            .get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        assert_eq!(attrs.content_size, Some(150));

        assert_eq!(
            file.read_at(100, 0).await.unwrap().map_err(zx::Status::err_from_raw).unwrap(),
            write_data,
        );
        assert_eq!(
            file.read_at(50, 100).await.unwrap().map_err(zx::Status::err_from_raw).unwrap(),
            vec![0; 50],
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_truncate_allocated_file_unaligned() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.allocate(0, PAGE_SIZE * 2, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let write_data = (0..20).cycle().take(PAGE_SIZE.get() as usize).collect::<Vec<_>>();
        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        file.resize(PAGE_SIZE + 100).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data,
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_complete_truncate_allocated_file() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        file.allocate(0, PAGE_SIZE * 2, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        let write_data = (0..20).cycle().take(PAGE_SIZE.get() as usize).collect::<Vec<_>>();
        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        file.resize(0).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        assert_eq!(
            file.write_at(&write_data, PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            PAGE_SIZE
        );
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        assert_eq!(
            file.read_at(PAGE_SIZE.get(), PAGE_SIZE.get())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap(),
            write_data,
        );

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_allocate_truncate_allocate() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let contents = vec![1; 15000];
        fuchsia_fs::file::write(&file, &contents).await.unwrap();
        file.allocate(0, 6000, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        file.resize(2000).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.allocate(14000, 4000, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_allocate_existing_data_no_sync() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let contents = vec![1; 10000];
        fuchsia_fs::file::write(&file, &contents).await.unwrap();
        {
            assert_eq!(
                file.seek(fio::SeekOrigin::Start, 0)
                    .await
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
                    .unwrap(),
                0
            );
            let data = fuchsia_fs::file::read(&file).await.unwrap();
            assert_eq!(contents.len(), data.len());
            assert_eq!(&contents, &data);
        }
        file.allocate(1000, 5000, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        {
            assert_eq!(
                file.seek(fio::SeekOrigin::Start, 0)
                    .await
                    .unwrap()
                    .map_err(zx::Status::err_from_raw)
                    .unwrap(),
                0
            );
            let data = fuchsia_fs::file::read(&file).await.unwrap();
            assert_eq!(contents.len(), data.len());
            assert_eq!(&contents, &data);
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_write_to_previously_allocated_range_between_flushes() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let contents = vec![1; 42007];
        fuchsia_fs::file::write(&file, &contents).await.unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.allocate(4125, 29053, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        file.resize(22932).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        let contents = vec![1; 7963];
        file.write_at(&contents, 22066).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        let contents = vec![1; 2697];
        file.write_at(&contents, 61919).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_truncate_then_allocate_between_syncs() {
        let fixture = TestFixture::new().await;
        let root = fixture.root();
        let file = open_file_checked(
            &root,
            FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;

        let contents = vec![1; 4096];
        fuchsia_fs::file::write(&file, &contents).await.unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.resize(0).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        file.allocate(0, 4096, fio::AllocateMode::empty())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap();
        file.sync().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        fixture.close().await;
    }

    // This test has to use two threads because we want to test two concurrent flushes.
    #[fuchsia::test(threads = 2)]
    async fn test_concurrent_flushes() {
        let fixture = TestFixture::new_unencrypted().await;

        {
            let root = fixture.root();
            let file = open_file_checked(
                &root,
                FILE_NAME,
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;

            // Write to the file and flush it so that we can truncate it.
            let contents = vec![1; 4096];
            fuchsia_fs::file::write(&file, &contents).await.unwrap();

            // Flush the write so that the truncate below sets up a pending shrink.
            file.sync().await.unwrap().unwrap();

            let barrier = Arc::new(std::sync::Barrier::new(2));

            let (file_proxy, server_end) = fidl::endpoints::create_sync_proxy::<fio::FileMarker>();
            file.clone(server_end.into_channel().into()).expect("clone failed");
            let barrier_clone = barrier.clone();
            let sync_finished = Arc::new(AtomicBool::new(false));
            let sync_finished_clone = sync_finished.clone();
            let sync_thread = std::thread::spawn(move || {
                // Wait until the first flush starts.
                barrier_clone.wait();

                // Issue another sync.  This should end up blocked behind the first flush.
                file_proxy.sync(zx::MonotonicInstant::INFINITE).unwrap().unwrap();

                sync_finished_clone.store(true, Ordering::Relaxed);
            });

            let once = AtomicBool::new(false);
            let _guard = CALLBACK_BEFORE_RANGE_COLLECTION.set(move || {
                if !once.swap(true, Ordering::Relaxed) {
                    // Synchronise with the barrier in the sync thread.
                    barrier.wait();

                    // Wait a short while to allow the `sync` call to run.
                    std::thread::sleep(std::time::Duration::from_millis(100));

                    // The second sync should wait for the first flush.
                    assert!(!sync_finished.load(Ordering::Relaxed));
                }
            });

            // Truncate it.  This sets up the flush so that when it runs, it will do the truncation
            // first which will update the mtime.  This will mean that the second flush will call
            // needs_flush, and except for a flush proceeding, it will look like no flush is
            // required.
            file.resize(100).await.unwrap().unwrap();

            // Write something that makes the file dirty.
            let contents = vec![1; 4096];
            fuchsia_fs::file::write(&file, &contents).await.unwrap();

            // Trigger the first flush which should end up in the callback above.
            file.sync().await.unwrap().unwrap();

            sync_thread.join().expect("sync thread failed");
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_flush_state_successful_flow() {
        let (fs, volume) = open_filesystem().await;
        let store_object_id = volume.volume().store().store_object_id();
        let allocator = fs.allocator();
        let needed = reservation_needed(10);
        let reservation = allocator.clone().reserve(Some(store_object_id), needed).unwrap();

        let marked_dirty_pages = DirtyPages { reserved: 10, unreserved: 5 };

        let mut flush_state = FlushState::new(reservation, marked_dirty_pages, FlushType::Sync);

        let dirty_pages_to_flush = DirtyPages { reserved: 5, unreserved: 2 };
        let dirty_pages_not_to_flush = DirtyPages { reserved: 5, unreserved: 3 };

        let result =
            flush_state.set_flush_batch_count(dirty_pages_to_flush, dirty_pages_not_to_flush);
        assert!(result.is_ok());

        flush_state.did_flush_pages(dirty_pages_to_flush);

        let inner = Inner::new(false);

        let pages_cleaned = flush_state.finish(&inner);

        assert_eq!(pages_cleaned, 7);

        {
            let mut inner_locked = inner.lock();
            assert_eq!(inner_locked.dirty_pages.reserved, 5);
            assert_eq!(inner_locked.dirty_pages.unreserved, 3);

            inner_locked.forget_dirty_pages(allocator, store_object_id);
        }

        fs.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_flush_state_finish_early_puts_back_dirty_pages() {
        let (fs, volume) = open_filesystem().await;
        let store_object_id = volume.volume().store().store_object_id();
        let allocator = fs.allocator();
        let needed = reservation_needed(10);
        let reservation = allocator.clone().reserve(Some(store_object_id), needed).unwrap();

        let marked_dirty_pages = DirtyPages { reserved: 10, unreserved: 5 };

        let flush_state = FlushState::new(reservation, marked_dirty_pages, FlushType::Sync);

        let inner = Inner::new(false);

        let pages_cleaned = flush_state.finish(&inner);

        assert_eq!(pages_cleaned, 0);

        {
            let mut inner_locked = inner.lock();
            assert_eq!(inner_locked.dirty_pages.reserved, 10);
            assert_eq!(inner_locked.dirty_pages.unreserved, 5);

            // Clean up reservation to avoid leak panic in allocator.
            inner_locked.forget_dirty_pages(allocator, store_object_id);
        }

        fs.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    async fn test_flush_state_finish_early_after_set_flush_batch_count_fails() {
        let (fs, volume) = open_filesystem().await;
        let store_object_id = volume.volume().store().store_object_id();
        let allocator = fs.allocator();
        let needed = reservation_needed(10);
        let reservation = allocator.clone().reserve(Some(store_object_id), needed).unwrap();

        let marked_dirty_pages = DirtyPages { reserved: 10, unreserved: 5 };

        let mut flush_state = FlushState::new(reservation, marked_dirty_pages, FlushType::Sync);

        let dirty_pages_to_flush = DirtyPages { reserved: 15, unreserved: 0 };
        let dirty_pages_not_to_flush = DirtyPages::default();

        let result =
            flush_state.set_flush_batch_count(dirty_pages_to_flush, dirty_pages_not_to_flush);
        assert!(result.is_err());

        let inner = Inner::new(false);

        let pages_cleaned = flush_state.finish(&inner);

        assert_eq!(pages_cleaned, 0);

        {
            let mut inner_locked = inner.lock();
            assert_eq!(inner_locked.dirty_pages.reserved, 10);
            assert_eq!(inner_locked.dirty_pages.unreserved, 5);

            inner_locked.forget_dirty_pages(allocator, store_object_id);
        }

        fs.close().await.expect("close filesystem failed");
    }

    #[fuchsia::test]
    #[should_panic(expected = "ReadyToFlush")]
    async fn test_flush_state_panic_if_take_extra_dirty_pages_not_called() {
        let (fs, volume) = open_filesystem().await;
        let store_object_id = volume.volume().store().store_object_id();
        let allocator = fs.allocator();
        let reservation = allocator.reserve_with(Some(store_object_id), |_| 0);

        let mut flush_state = FlushState::new(reservation, DirtyPages::default(), FlushType::Sync);

        let dirty_pages_to_flush = DirtyPages { reserved: 5, unreserved: 0 };
        let dirty_pages_not_to_flush = DirtyPages::default();

        let _ = flush_state.set_flush_batch_count(dirty_pages_to_flush, dirty_pages_not_to_flush);

        flush_state.did_flush_pages(DirtyPages::default());
    }

    #[fuchsia::test]
    #[should_panic(expected = "ReadyToFlush")]
    async fn test_flush_state_skipping_steps_panics() {
        let (fs, volume) = open_filesystem().await;
        let store_object_id = volume.volume().store().store_object_id();
        let allocator = fs.allocator();
        let reservation = allocator.reserve_with(Some(store_object_id), |_| 0);

        let mut flush_state = FlushState::new(reservation, DirtyPages::default(), FlushType::Sync);

        flush_state.did_flush_pages(DirtyPages::default());
    }

    #[fuchsia::test]
    async fn test_drop_last_opened_node_in_mark_dirty() {
        let (fs, volume) = open_filesystem().await;
        let root = open_volume(&volume);
        {
            let file = open_file_checked(
                &root,
                FILE_NAME,
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE
                    | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
            )
            .await;
            file.resize(PAGE_SIZE.get()).await.unwrap().expect("Resizing");
            let file_id = file
                .get_attributes(fio::NodeAttributesQuery::ID)
                .await
                .unwrap()
                .unwrap()
                .1
                .id
                .unwrap();

            // Retrieve the internal FxFile object from the volume's node cache.
            let node = volume.volume().cache().get(file_id).expect("Node not in cache");
            let fx_file = node.into_any().downcast::<FxFile>().expect("Not FxFile");

            // Hold open the last copy here.
            let opened_node = fx_file.clone().into_opened_node().expect("into_opened_node failed");
            close_file_checked(file).await;

            // Create a dirty range to synchronously call MarkDirty on the file with the last opened
            // reference.
            let range = MarkDirtyRange::new(0..PAGE_SIZE.get(), opened_node);
            fx_file.mark_dirty(range);
        }

        close_dir_checked(root).await;
        volume.volume().terminate().await;
        fs.close().await.expect("close filesystem failed");
    }
}
