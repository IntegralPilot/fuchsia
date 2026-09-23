// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::checksum::{Checksum, Checksums};
use crate::errors::FxfsError;
use crate::log::*;
use crate::lsm_tree::Query;
use crate::lsm_tree::merge::{Merger, MergerIterator};
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::ObjectHandle;
use crate::object_store::extent_record::{ExtentMode, ExtentValue};
use crate::object_store::object_manager::ObjectManager;
use crate::object_store::object_record::{
    AttributeKey, ExtendedAttributeValue, ObjectAttributes, ObjectKey, ObjectKeyData, ObjectValue,
    Timestamp,
};
use crate::object_store::transaction::{
    AssocObj, AssociatedObject, LockKey, Mutation, ObjectStoreMutation, Options, ReadGuard,
    Transaction, lock_keys,
};
use crate::object_store::{
    AttributeId, Extent, FileExtent, HandleOptions, HandleOwner, ObjectStore, TrimMode, TrimResult,
    VOLUME_DATA_KEY_ID,
};
use crate::range::RangeExt;
use anyhow::{Context, Error, anyhow, bail, ensure};
use assert_matches::assert_matches;
use bit_vec::BitVec;
use futures::stream::{FuturesOrdered, FuturesUnordered, unfold};
use futures::{Stream, TryStreamExt, try_join};
use fxfs_crypto::{
    Cipher, CipherHolder, CipherSet, EncryptionKey, FindKeyResult, FxfsCipher, KeyPurpose,
    MutPtrByteSlice,
};
use fxfs_trace::{TraceFutureExt, trace, trace_future_args};
use static_assertions::const_assert;
use std::cmp::min;
use std::future::Future;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool, Ordering};
use storage_device::buffer::{Buffer, BufferFuture, BufferRef, MutableBufferRef};
use storage_device::{InlineCryptoOptions, ReadOptions, WriteFlags, WriteOptions};
use storage_units::BlockSize;

use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;

/// Maximum size for an extended attribute name.
pub const MAX_XATTR_NAME_SIZE: usize = 255;
/// Maximum size an extended attribute can be before it's stored in an object attribute instead of
/// inside the record directly.
pub const MAX_INLINE_XATTR_SIZE: usize = 256;
/// Maximum size for an extended attribute value. NB: the maximum size for an extended attribute is
/// 64kB, which we rely on for correctness when deleting attributes, so ensure it's always
/// enforced.
pub const MAX_XATTR_VALUE_SIZE: usize = 64000;

/// Zeroes blocks in 'buffer' based on `bitmap`, one bit per block from start of buffer.
fn apply_bitmap_zeroing(
    block_size: BlockSize,
    bitmap: &bit_vec::BitVec,
    mut buffer: MutableBufferRef<'_>,
) {
    let mut buf = buffer.as_mut_ptr_slice();
    debug_assert_eq!(bitmap.len() as u64 * block_size, buf.len() as u64);
    for (i, block) in bitmap.iter().enumerate() {
        if !block {
            let start = (i as u64 * block_size) as usize;
            buf.subslice_mut(start..start + block_size.get() as usize).fill(0);
        }
    }
}

/// When writing, often the logic should be generic over whether or not checksums are generated.
/// This provides that and a handy way to convert to the more general ExtentMode that eventually
/// stores it on disk.
#[derive(Debug, Clone, PartialEq)]
pub enum MaybeChecksums {
    None,
    Fletcher(Vec<Checksum>),
}

impl MaybeChecksums {
    pub fn maybe_as_ref(&self) -> Option<&[Checksum]> {
        match self {
            Self::None => None,
            Self::Fletcher(sums) => Some(&sums),
        }
    }

    pub fn split_off(&mut self, at: usize) -> Self {
        match self {
            Self::None => Self::None,
            Self::Fletcher(sums) => Self::Fletcher(sums.split_off(at)),
        }
    }

    pub fn to_mode(self) -> ExtentMode {
        match self {
            Self::None => ExtentMode::Raw,
            Self::Fletcher(sums) => ExtentMode::Cow(Checksums::fletcher(sums)),
        }
    }

    pub fn into_option(self) -> Option<Vec<Checksum>> {
        match self {
            Self::None => None,
            Self::Fletcher(sums) => Some(sums),
        }
    }
}

/// The mode of operation when setting extended attributes. This is the same as the fidl definition
/// but is replicated here so we don't have fuchsia.io structures in the api, so this can be used
/// on host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetExtendedAttributeMode {
    /// Create the extended attribute if it doesn't exist, replace the value if it does.
    Set,
    /// Create the extended attribute if it doesn't exist, fail if it does.
    Create,
    /// Replace the extended attribute value if it exists, fail if it doesn't.
    Replace,
}

impl From<fio::SetExtendedAttributeMode> for SetExtendedAttributeMode {
    fn from(other: fio::SetExtendedAttributeMode) -> SetExtendedAttributeMode {
        match other {
            fio::SetExtendedAttributeMode::Set => SetExtendedAttributeMode::Set,
            fio::SetExtendedAttributeMode::Create => SetExtendedAttributeMode::Create,
            fio::SetExtendedAttributeMode::Replace => SetExtendedAttributeMode::Replace,
        }
    }
}

enum Encryption {
    /// The object doesn't use encryption.
    None,

    /// The object has keys that are cached (which means unwrapping occurs on-demand) with
    /// KeyManager.
    CachedKeys,

    /// The object has permanent keys registered with KeyManager.
    PermanentKeys,
}

#[derive(PartialEq, Debug)]
enum OverwriteBitmaps {
    None,
    Some {
        /// The block bitmap of a partial overwrite extent in the tree.
        extent_bitmap: BitVec,
        /// A bitmap of the blocks written to by the current overwrite.
        write_bitmap: BitVec,
        /// BitVec doesn't have a slice equivalent, so for a particular section of the write we
        /// keep track of an offset in the bitmaps to operate on.
        bitmap_offset: usize,
    },
}

impl OverwriteBitmaps {
    fn new(extent_bitmap: BitVec) -> Self {
        OverwriteBitmaps::Some {
            write_bitmap: BitVec::from_elem(extent_bitmap.len(), false),
            extent_bitmap,
            bitmap_offset: 0,
        }
    }

    fn is_none(&self) -> bool {
        *self == OverwriteBitmaps::None
    }

    fn set_offset(&mut self, new_offset: usize) {
        match self {
            OverwriteBitmaps::None => (),
            OverwriteBitmaps::Some { bitmap_offset, .. } => *bitmap_offset = new_offset,
        }
    }

    fn get_from_extent_bitmap(&self, i: usize) -> Option<bool> {
        match self {
            OverwriteBitmaps::None => None,
            OverwriteBitmaps::Some { extent_bitmap, bitmap_offset, .. } => {
                extent_bitmap.get(*bitmap_offset + i)
            }
        }
    }

    fn set_in_write_bitmap(&mut self, i: usize, x: bool) {
        match self {
            OverwriteBitmaps::None => (),
            OverwriteBitmaps::Some { write_bitmap, bitmap_offset, .. } => {
                write_bitmap.set(*bitmap_offset + i, x)
            }
        }
    }

    fn take_bitmaps(self) -> Option<(BitVec, BitVec)> {
        match self {
            OverwriteBitmaps::None => None,
            OverwriteBitmaps::Some { extent_bitmap, write_bitmap, .. } => {
                Some((extent_bitmap, write_bitmap))
            }
        }
    }
}

/// When writing to Overwrite ranges, we need to emit whether a set of checksums for a device range
/// is the first write to that region or not. This tracks one such range so we can use it after the
/// write to break up the returned checksum list.
#[derive(PartialEq, Debug)]
struct ChecksumRangeChunk {
    checksum_range: Range<usize>,
    device_range: Range<u64>,
    is_first_write: bool,
}

impl ChecksumRangeChunk {
    fn group_first_write_ranges(
        bitmaps: &mut OverwriteBitmaps,
        block_size: BlockSize,
        write_device_range: Range<u64>,
    ) -> Vec<ChecksumRangeChunk> {
        let write_block_len = (write_device_range.length().unwrap() / block_size) as usize;
        if bitmaps.is_none() {
            // If there is no bitmap, then the overwrite range is fully written to. However, we
            // could still be within the journal flush window where one of the blocks was written
            // to for the first time to put it in this state, so we still need to emit the
            // checksums in case replay needs them.
            vec![ChecksumRangeChunk {
                checksum_range: 0..write_block_len,
                device_range: write_device_range,
                is_first_write: false,
            }]
        } else {
            let mut checksum_ranges = vec![ChecksumRangeChunk {
                checksum_range: 0..0,
                device_range: write_device_range.start..write_device_range.start,
                is_first_write: !bitmaps.get_from_extent_bitmap(0).unwrap(),
            }];
            let mut working_range = checksum_ranges.last_mut().unwrap();
            for i in 0..write_block_len {
                bitmaps.set_in_write_bitmap(i, true);

                // bitmap.get returning true means the block is initialized and therefore has been
                // written to before.
                if working_range.is_first_write != bitmaps.get_from_extent_bitmap(i).unwrap() {
                    // is_first_write is tracking opposite of what comes back from the bitmap, so
                    // if the are still opposites we continue our current range.
                    working_range.checksum_range.end += 1;
                    working_range.device_range.end += block_size;
                } else {
                    // If they are the same, then we need to make a new chunk.
                    let new_chunk = ChecksumRangeChunk {
                        checksum_range: working_range.checksum_range.end
                            ..working_range.checksum_range.end + 1,
                        device_range: working_range.device_range.end
                            ..working_range.device_range.end + block_size,
                        is_first_write: !working_range.is_first_write,
                    };
                    checksum_ranges.push(new_chunk);
                    working_range = checksum_ranges.last_mut().unwrap();
                }
            }
            checksum_ranges
        }
    }
}

/// StoreObjectHandle is the lowest-level, untyped handle to an object with the id [`object_id`] in
/// a particular store, [`owner`]. It provides functionality shared across all objects, such as
/// reading and writing attributes and managing encryption keys.
///
/// Since it's untyped, it doesn't do any object kind validation, and is generally meant to
/// implement higher-level typed handles.
///
/// For file-like objects with a data attribute, DataObjectHandle implements traits and helpers for
/// doing more complex extent management and caches the content size.
///
/// For directory-like objects, Directory knows how to add and remove child objects and enumerate
/// its children.
pub struct StoreObjectHandle<S: HandleOwner> {
    owner: Arc<S>,
    object_id: u64,
    options: HandleOptions,
    trace: AtomicBool,
    encryption: Encryption,
}

impl<S: HandleOwner> ObjectHandle for StoreObjectHandle<S> {
    fn set_trace(&self, v: bool) {
        info!(store_id = self.store().store_object_id, oid = self.object_id(), trace = v; "trace");
        self.trace.store(v, atomic::Ordering::Relaxed);
    }

    fn object_id(&self) -> u64 {
        return self.object_id;
    }

    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.store().device.allocate_buffer(size)
    }

    fn block_size(&self) -> BlockSize {
        self.store().block_size()
    }
}

struct Watchdog {
    _task: fasync::Task<()>,
}

impl Watchdog {
    fn new(increment_seconds: u64, cb: impl Fn(u64) + Send + 'static) -> Self {
        Self {
            _task: fasync::Task::spawn(
                async move {
                    let increment = increment_seconds.try_into().unwrap();
                    let mut fired_counter = 0;
                    let mut next_wake = fasync::MonotonicInstant::now();
                    loop {
                        next_wake += std::time::Duration::from_secs(increment).into();
                        // If this isn't being scheduled this will purposely result in fast looping
                        // when it does. This will be insightful about the state of the thread and
                        // task scheduling.
                        if fasync::MonotonicInstant::now() < next_wake {
                            fasync::Timer::new(next_wake).await;
                        }
                        fired_counter += 1;
                        cb(fired_counter);
                    }
                }
                .trace(trace_future_args!("StoreObjectHandle::Watchdog")),
            ),
        }
    }
}

impl<S: HandleOwner> StoreObjectHandle<S> {
    /// Make a new StoreObjectHandle for the object with id [`object_id`] in store [`owner`].
    pub fn new(
        owner: Arc<S>,
        object_id: u64,
        permanent_keys: bool,
        options: HandleOptions,
        trace: bool,
    ) -> Self {
        let encryption = if permanent_keys {
            Encryption::PermanentKeys
        } else if owner.as_ref().as_ref().is_encrypted() {
            Encryption::CachedKeys
        } else {
            Encryption::None
        };
        Self { owner, object_id, encryption, options, trace: AtomicBool::new(trace) }
    }

    pub fn owner(&self) -> &Arc<S> {
        &self.owner
    }

    pub fn store(&self) -> &ObjectStore {
        self.owner.as_ref().as_ref()
    }

    pub fn trace(&self) -> bool {
        self.trace.load(atomic::Ordering::Relaxed)
    }

    pub fn is_encrypted(&self) -> bool {
        !matches!(self.encryption, Encryption::None)
    }

    /// Get the default set of transaction options for this object. This is mostly the overall
    /// default, modified by any [`HandleOptions`] held by this handle.
    pub fn default_transaction_options<'b>(&self) -> Options<'b> {
        Options { skip_journal_checks: self.options.skip_journal_checks, ..Default::default() }
    }

    pub async fn new_transaction_with_options<'b>(
        &self,
        attribute_id: AttributeId,
        options: Options<'b>,
    ) -> Result<Transaction<'b>, Error> {
        Ok(self
            .store()
            .new_transaction(
                lock_keys![
                    LockKey::object_attribute(
                        self.store().store_object_id(),
                        self.object_id(),
                        attribute_id,
                    ),
                    LockKey::object(self.store().store_object_id(), self.object_id()),
                ],
                options,
            )
            .await?)
    }

    pub async fn new_transaction<'b>(
        &self,
        attribute_id: AttributeId,
    ) -> Result<Transaction<'b>, Error> {
        self.new_transaction_with_options(attribute_id, self.default_transaction_options()).await
    }

    // If |transaction| has an impending mutation for the underlying object, returns that.
    // Otherwise, looks up the object from the tree.
    async fn txn_get_object_mutation(
        &self,
        transaction: &Transaction<'_>,
    ) -> Result<ObjectStoreMutation, Error> {
        self.store().txn_get_object_mutation(transaction, self.object_id()).await
    }

    // Returns the amount deallocated.
    async fn deallocate_old_extents(
        &self,
        transaction: &mut Transaction<'_>,
        attribute_id: AttributeId,
        range: Range<u64>,
    ) -> Result<u64, Error> {
        let block_size = self.block_size();
        assert!(block_size.is_aligned(&range));
        if range.start == range.end {
            return Ok(0);
        }
        let tree = &self.store().tree;
        let layer_set = tree.layer_set();
        let key = Extent(range);
        let lower_bound = ObjectKey::attribute(
            self.object_id(),
            attribute_id,
            AttributeKey::Extent(key.search_key()),
        );
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullRange(&lower_bound)).await?;
        let allocator = self.store().allocator();
        let mut deallocated = 0;
        let trace = self.trace();
        while let Some(ItemRef {
            key:
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(attr_id, AttributeKey::Extent(extent_key)),
                },
            value: ObjectValue::Extent(value),
            ..
        }) = iter.get()
        {
            if *object_id != self.object_id() || *attr_id != attribute_id {
                break;
            }
            if let ExtentValue::Some { device_offset, .. } = value {
                if let Some(overlap) = key.overlap(extent_key) {
                    let range = device_offset + overlap.start - extent_key.start
                        ..device_offset + overlap.end - extent_key.start;
                    ensure!(block_size.is_aligned(&range), FxfsError::Inconsistent);
                    if trace {
                        info!(
                            store_id = self.store().store_object_id(),
                            oid = self.object_id(),
                            device_range:? = range,
                            len = range.end - range.start,
                            extent_key:?;
                            "D",
                        );
                    }
                    allocator
                        .deallocate(transaction, self.store().store_object_id(), range)
                        .await?;
                    deallocated += overlap.end - overlap.start;
                } else {
                    break;
                }
            }
            iter.advance().await?;
        }
        Ok(deallocated)
    }

    // Writes aligned data (that should already be encrypted) to the given offset and computes
    // checksums if requested. The aligned data must be from a single logical file range.
    //
    // `flags` are forwarded to the underlying device as `WriteOptions::flags` (e.g.
    // `WriteFlags::PRE_BARRIER`).
    async fn write_aligned(
        &self,
        buf: BufferRef<'_>,
        device_offset: u64,
        crypt_ctx: Option<(u64, u8)>,
        flags: WriteFlags,
    ) -> Result<MaybeChecksums, Error> {
        if self.trace() {
            info!(
                store_id = self.store().store_object_id(),
                oid = self.object_id(),
                device_range:? = (device_offset..device_offset + buf.len() as u64),
                len = buf.len();
                "W",
            );
        }
        let store = self.store();
        store.device_write_ops.fetch_add(1, Ordering::Relaxed);
        let _watchdog = Watchdog::new(10, |count| {
            warn!("Write I/O request blocked for {} seconds", count * 10);
        });

        let (opts, compute_checksums) = match crypt_ctx {
            Some((dun, slot)) => {
                if !store.filesystem().options().barriers_enabled {
                    return Err(anyhow!(FxfsError::InvalidArgs)
                        .context("Barriers must be enabled for inline encrypted writes."));
                }
                (
                    WriteOptions { inline_crypto: InlineCryptoOptions::enabled(slot, dun), flags },
                    false,
                )
            }
            None => (WriteOptions { flags, ..Default::default() }, !self.options.skip_checksums),
        };

        if compute_checksums {
            let mut checksums = Vec::new();
            try_join!(store.device.write_with_opts(device_offset, buf, opts), async {
                let block_size = self.block_size().get() as usize;
                for chunk in buf.as_ptr_slice().chunks(block_size) {
                    checksums.push(crate::checksum::fletcher64_ptr(chunk, 0));
                }
                Ok(())
            })?;
            Ok(MaybeChecksums::Fletcher(checksums))
        } else {
            store.device.write_with_opts(device_offset, buf, opts).await?;
            Ok(MaybeChecksums::None)
        }
    }

    /// Flushes the underlying device.  This is expensive and should be used sparingly.
    pub async fn flush_device(&self) -> Result<(), Error> {
        self.store().device().flush().await
    }

    pub async fn update_allocated_size(
        &self,
        transaction: &mut Transaction<'_>,
        allocated: u64,
        deallocated: u64,
    ) -> Result<(), Error> {
        if allocated == deallocated {
            return Ok(());
        }
        let mut mutation = self.txn_get_object_mutation(transaction).await?;
        if let ObjectValue::Object {
            attributes: ObjectAttributes { project_id, allocated_size, .. },
            ..
        } = &mut mutation.item.value
        {
            // The only way for these to fail are if the volume is inconsistent.
            *allocated_size = allocated_size
                .checked_add(allocated)
                .ok_or_else(|| anyhow!(FxfsError::Inconsistent).context("Allocated size overflow"))?
                .checked_sub(deallocated)
                .ok_or_else(|| {
                    anyhow!(FxfsError::Inconsistent).context("Allocated size underflow")
                })?;

            if let Some(project_id) = project_id {
                // The allocated and deallocated shouldn't exceed the max size of the file which is
                // bound within i64.
                let diff = i64::try_from(allocated).unwrap() - i64::try_from(deallocated).unwrap();
                transaction.add(
                    self.store().store_object_id(),
                    Mutation::merge_object(
                        ObjectKey::project_usage(
                            self.store().root_directory_object_id(),
                            *project_id,
                        ),
                        ObjectValue::BytesAndNodes { bytes: diff, nodes: 0 },
                    ),
                );
            }
        } else {
            // This can occur when the object mutation is created from an object in the tree which
            // was corrupt.
            bail!(anyhow!(FxfsError::Inconsistent).context("Unexpected object value"));
        }
        transaction.add(self.store().store_object_id, Mutation::ObjectStore(mutation));
        Ok(())
    }

    pub async fn update_attributes<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        node_attributes: Option<&fio::MutableNodeAttributes>,
        change_time: Option<Timestamp>,
    ) -> Result<(), Error> {
        if let Some(&fio::MutableNodeAttributes { selinux_context: Some(ref context), .. }) =
            node_attributes
        {
            if let fio::SelinuxContext::Data(context) = context {
                self.set_extended_attribute_impl(
                    "security.selinux".into(),
                    context.clone(),
                    SetExtendedAttributeMode::Set,
                    transaction,
                )
                .await?;
            } else {
                return Err(anyhow!(FxfsError::InvalidArgs)
                    .context("Only set SELinux context with `data` member."));
            }
        }
        self.store()
            .update_attributes(transaction, self.object_id, node_attributes, change_time)
            .await
    }

    /// Zeroes the given range.  The range must be aligned.  Returns the amount of data deallocated.
    pub async fn zero(
        &self,
        transaction: &mut Transaction<'_>,
        attribute_id: AttributeId,
        range: Range<u64>,
    ) -> Result<(), Error> {
        let deallocated =
            self.deallocate_old_extents(transaction, attribute_id, range.clone()).await?;
        if deallocated > 0 {
            self.update_allocated_size(transaction, 0, deallocated).await?;
            transaction.add(
                self.store().store_object_id,
                Mutation::merge_object(
                    ObjectKey::extent(self.object_id(), attribute_id, range),
                    ObjectValue::Extent(ExtentValue::deleted_extent()),
                ),
            );
        }
        Ok(())
    }

    // Returns a new aligned buffer (reading the head and tail blocks if necessary) with a copy of
    // the data from `buf`.
    pub async fn align_buffer(
        &self,
        attribute_id: AttributeId,
        offset: u64,
        buf: BufferRef<'_>,
    ) -> Result<(std::ops::Range<u64>, Buffer<'_>), Error> {
        let block_size = self.block_size();
        let end = offset + buf.len() as u64;
        let aligned = block_size.align_range_outwards(offset..end).ok_or(FxfsError::TooBig)?;

        let mut aligned_buf =
            self.store().device.allocate_buffer((aligned.end - aligned.start) as usize).await;

        // Deal with head alignment.
        if aligned.start < offset {
            let mut head_block = aligned_buf.subslice_mut(0..block_size.get() as usize);
            let read = self.read(attribute_id, aligned.start, head_block.reborrow()).await?;
            let len = head_block.len();
            head_block.subslice_mut(read..len).fill(0);
        }

        // Deal with tail alignment.
        if aligned.end > end {
            let end_block_offset = aligned.end - block_size;
            // There's no need to read the tail block if we read it as part of the head block.
            if offset <= end_block_offset {
                let mut tail_block =
                    aligned_buf.subslice_mut(aligned_buf.len() - block_size.get() as usize..);
                let read = self.read(attribute_id, end_block_offset, tail_block.reborrow()).await?;
                let len = tail_block.len();
                tail_block.subslice_mut(read..len).fill(0);
            }
        }

        aligned_buf
            .subslice_mut((offset - aligned.start) as usize..(end - aligned.start) as usize)
            .copy_from_buffer(buf);

        Ok((aligned, aligned_buf))
    }

    /// Trim an attribute's extents, potentially adding a graveyard trim entry if more trimming is
    /// needed, so the transaction can be committed without worrying about leaking data.
    ///
    /// This doesn't update the size stored in the attribute value - the caller is responsible for
    /// doing that to keep the size up to date.
    pub async fn shrink(
        &self,
        transaction: &mut Transaction<'_>,
        attribute_id: AttributeId,
        size: u64,
    ) -> Result<NeedsTrim, Error> {
        let store = self.store();
        let needs_trim = matches!(
            store
                .trim_some(transaction, self.object_id(), attribute_id, TrimMode::FromOffset(size))
                .await?,
            TrimResult::Incomplete
        );
        if needs_trim {
            // Add the object to the graveyard in case the following transactions don't get
            // replayed.
            let graveyard_id = store.graveyard_directory_object_id();
            // Check if the object is already in the graveyard.
            let in_graveyard = store
                .tree
                .find_map(&ObjectKey::graveyard_entry(graveyard_id, self.object_id()), |item| {
                    matches!(item.value, ObjectValue::Some | ObjectValue::Trim)
                })
                .await?
                .unwrap_or(false);
            if !in_graveyard {
                transaction.add(
                    store.store_object_id,
                    Mutation::replace_or_insert_object(
                        ObjectKey::graveyard_entry(graveyard_id, self.object_id()),
                        ObjectValue::Trim,
                    ),
                );
            }
        }
        Ok(NeedsTrim(needs_trim))
    }

    /// Reads and decrypts a singular logical range.
    pub async fn read_and_decrypt(
        &self,
        attribute_id: AttributeId,
        device_offset: u64,
        file_offset: u64,
        mut buffer: MutableBufferRef<'_>,
        key_id: u64,
    ) -> Result<(), Error> {
        let store = self.store();
        store.device_read_ops.fetch_add(1, Ordering::Relaxed);

        let _watchdog = Watchdog::new(10, |count| {
            warn!("Read I/O request blocked for {} seconds", count * 10);
        });

        let (_key_id, key) = self.get_key(Some(key_id)).await?;
        if let Some(key) = key {
            if let Some((dun, slot)) =
                key.crypt_ctx(self.object_id, attribute_id.raw(), file_offset)
            {
                store
                    .device
                    .read_with_opts(
                        device_offset as u64,
                        buffer.reborrow(),
                        ReadOptions { inline_crypto: InlineCryptoOptions::enabled(slot, dun) },
                    )
                    .await?;
            } else {
                store.device.read(device_offset, buffer.reborrow()).await?;
                key.decrypt(
                    self.object_id,
                    attribute_id.raw(),
                    device_offset,
                    file_offset,
                    buffer.as_mut_ptr_slice(),
                )?;
            }
        } else {
            store.device.read(device_offset, buffer.reborrow()).await?;
        }

        Ok(())
    }

    /// Returns the specified key. If `key_id` is None, it will try and return the fscrypt key if
    /// it is present, or the volume key if it isn't. If the fscrypt key is present, but the key
    /// cannot be unwrapped, then this will return `FxfsError::NoKey`. If the volume is not
    /// encrypted, this returns None.
    pub async fn get_key(
        &self,
        key_id: Option<u64>,
    ) -> Result<(u64, Option<Arc<dyn Cipher>>), Error> {
        let store = self.store();
        let result = match self.encryption {
            Encryption::None => (VOLUME_DATA_KEY_ID, None),
            Encryption::CachedKeys => {
                if let Some(key_id) = key_id {
                    (
                        key_id,
                        Some(
                            store
                                .key_manager
                                .get_key(
                                    self.object_id,
                                    store.crypt().ok_or_else(|| anyhow!("No crypt!"))?.as_ref(),
                                    async || store.get_keys(self.object_id).await,
                                    key_id,
                                )
                                .await?,
                        ),
                    )
                } else {
                    let (key_id, key) = store
                        .key_manager
                        .get_fscrypt_key_if_present(
                            self.object_id,
                            store.crypt().ok_or_else(|| anyhow!("No crypt!"))?.as_ref(),
                            async || store.get_keys(self.object_id).await,
                        )
                        .await?;
                    (key_id, Some(key))
                }
            }
            Encryption::PermanentKeys => {
                (VOLUME_DATA_KEY_ID, Some(store.key_manager.get(self.object_id).await?.unwrap()))
            }
        };

        // Ensure that if the key we receive uses inline encryption, barriers should be enabled.
        if let Some(ref key) = result.1 {
            if key.crypt_ctx(self.object_id, 0, 0).is_some() {
                if !store.filesystem().options().barriers_enabled {
                    return Err(anyhow!(FxfsError::InvalidArgs)
                        .context("Barriers must be enabled for inline encrypted writes."));
                }
            }
        }

        Ok(result)
    }

    /// Returns a stream of extents for the given attribute ID, mapping logical offsets to device
    /// offsets. Extents are sorted by logical offset and non-overlapping. Extents with no physical
    /// backing are skipped.
    pub async fn extent_stream<'a, 'b>(
        &'a self,
        merger: &'b mut Merger<'a, ObjectKey, ObjectValue>,
        attribute_id: AttributeId,
    ) -> Result<impl Stream<Item = Result<FileExtent, Error>> + 'b, Error> {
        let object_id = self.object_id();
        let iter = merger
            .query(Query::FullRange(&ObjectKey::attribute(
                object_id,
                attribute_id,
                AttributeKey::Extent(Extent::search_key_from_offset(0)),
            )))
            .await?;
        Ok(unfold(
            (iter, object_id, attribute_id),
            |(mut iter, object_id, attribute_id)| async move {
                loop {
                    match iter.get() {
                        Some(ItemRef {
                            key:
                                ObjectKey {
                                    object_id: id,
                                    data:
                                        ObjectKeyData::Attribute(
                                            attr_id,
                                            AttributeKey::Extent(extent_key),
                                        ),
                                },
                            value: ObjectValue::Extent(extent_value),
                            ..
                        }) if *id == object_id && *attr_id == attribute_id => {
                            let logical_range = extent_key.0.clone();
                            let device_range = match extent_value {
                                ExtentValue::Some { device_offset, .. } => {
                                    let len = logical_range.end - logical_range.start;
                                    Some(*device_offset..*device_offset + len)
                                }
                                // Extent with no physical backing (e.g. deleted extent).
                                ExtentValue::None => None,
                            };

                            // Advance the iterator before returning so the next invocation sees the
                            // following entry.
                            if let Err(e) = iter.advance().await {
                                return Some((Err(e.into()), (iter, object_id, attribute_id)));
                            }

                            if let Some(device_range) = device_range {
                                return Some((
                                    Ok(FileExtent::new(logical_range.start, device_range).unwrap()),
                                    (iter, object_id, attribute_id),
                                ));
                            } else {
                                // Skip extents with no physical backing; loop to check the next
                                // entry.
                                continue;
                            }
                        }
                        // No more entries matching this object_id and attribute_id.
                        _ => return None,
                    }
                }
            },
        ))
    }

    /// This will only work for a non-permanent volume data key. This is designed to be used with
    /// extended attributes where we'll only create the key on demand for directories and encrypted
    /// files.
    async fn get_or_create_key(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<Arc<dyn Cipher>, Error> {
        let store = self.store();

        // Fast path: try and get keys from the cache.
        if let Some(key) = store.key_manager.get(self.object_id).await.context("get failed")? {
            return Ok(key);
        }

        let crypt = store.crypt().ok_or_else(|| anyhow!("No crypt!"))?;

        // Next, see if the keys are already created.
        let (mut encryption_keys, mut cipher_set) = if let Some(value) =
            store.tree.find_value(&ObjectKey::keys(self.object_id)).await.context("find failed")?
        {
            if let ObjectValue::Keys(encryption_keys) = value {
                let cipher_set = store
                    .key_manager
                    .get_keys(
                        self.object_id,
                        crypt.as_ref(),
                        &mut Some(async || Ok(encryption_keys.clone())),
                        /* permanent= */ false,
                        /* force= */ false,
                    )
                    .await
                    .context("get_keys failed")?;
                match cipher_set.find_key(VOLUME_DATA_KEY_ID) {
                    FindKeyResult::NotFound => {}
                    FindKeyResult::Unavailable => return Err(FxfsError::NoKey.into()),
                    FindKeyResult::Key(key) => return Ok(key),
                }
                (encryption_keys, (*cipher_set).clone())
            } else {
                return Err(anyhow!(FxfsError::Inconsistent));
            }
        } else {
            Default::default()
        };

        // Proceed to create the key.  The transaction holds the required locks.
        let (key, unwrapped_key) = crypt.create_key(self.object_id, KeyPurpose::Data).await?;
        let cipher: Arc<dyn Cipher> = Arc::new(FxfsCipher::new(&unwrapped_key));

        // Add new cipher to cloned cipher set. This will replace existing one
        // if transaction is successful.
        cipher_set.add_key(VOLUME_DATA_KEY_ID, CipherHolder::Cipher(cipher.clone()));
        let cipher_set = Arc::new(cipher_set);

        // Arrange for the CipherSet to be added to the cache when (and if) the transaction
        // commits.
        struct UnwrappedKeys {
            object_id: u64,
            new_keys: Arc<CipherSet>,
        }

        impl AssociatedObject for UnwrappedKeys {
            fn will_apply_mutation(
                &self,
                _mutation: &Mutation,
                object_id: u64,
                manager: &ObjectManager,
            ) {
                manager.store(object_id).unwrap().key_manager.insert(
                    self.object_id,
                    self.new_keys.clone(),
                    /* permanent= */ false,
                );
            }
        }

        encryption_keys.insert(VOLUME_DATA_KEY_ID, EncryptionKey::Fxfs(key).into());

        transaction.add_with_object(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::keys(self.object_id),
                ObjectValue::keys(encryption_keys),
            ),
            AssocObj::Owned(Box::new(UnwrappedKeys {
                object_id: self.object_id,
                new_keys: cipher_set,
            })),
        );

        Ok(cipher)
    }

    pub async fn read(
        &self,
        attribute_id: AttributeId,
        offset: u64,
        mut buf: MutableBufferRef<'_>,
    ) -> Result<usize, Error> {
        let fs = self.store().filesystem();
        let guard = fs
            .lock_manager()
            .read_lock(lock_keys![LockKey::object_attribute(
                self.store().store_object_id(),
                self.object_id(),
                attribute_id,
            )])
            .await;

        let key = ObjectKey::attribute(self.object_id(), attribute_id, AttributeKey::Attribute);
        let size = self
            .store()
            .tree()
            .find_map(&key, |item| match item.value {
                ObjectValue::Attribute { size, .. } => Ok(*size),
                _ => Err(anyhow!(FxfsError::Inconsistent)),
            })
            .await?
            .transpose()?
            .unwrap_or(0);
        if offset >= size {
            return Ok(0);
        }
        let length = min(buf.len() as u64, size - offset) as usize;
        buf = buf.subslice_mut(0..length);
        self.read_unchecked(attribute_id, offset, buf, &guard).await?;
        Ok(length)
    }

    /// Read `buf.len()` bytes from the attribute `attribute_id`, starting at `offset`, into `buf`.
    /// It's required that a read lock on this attribute id is taken before this is called.
    ///
    /// This function doesn't do any size checking - any portion of `buf` past the end of the file
    /// will be filled with zeros. The caller is responsible for enforcing the file size on reads.
    /// This is because, just looking at the extents, we can't tell the difference between the file
    /// actually ending and there just being a section at the end with no data (since attributes
    /// are sparse).
    pub async fn read_unchecked(
        &self,
        attribute_id: AttributeId,
        mut offset: u64,
        mut buf: MutableBufferRef<'_>,
        _guard: &ReadGuard<'_>,
    ) -> Result<(), Error> {
        if buf.len() == 0 {
            return Ok(());
        }
        let end_offset = offset + buf.len() as u64;

        self.store().logical_read_ops.fetch_add(1, Ordering::Relaxed);

        // Whilst the read offset must be aligned to the filesystem block size, the buffer need only
        // be aligned to the device's block size.
        let block_size = self.block_size();
        let device_block_size = self.store().device.block_size() as u64;
        assert_eq!(offset % block_size, 0);
        assert_eq!(buf.range().start as u64 % device_block_size, 0);
        let tree = &self.store().tree;
        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::LimitedRange(&ObjectKey::extent(
                self.object_id(),
                attribute_id,
                offset..end_offset,
            )))
            .await?;
        let end_align = ((offset + buf.len() as u64) % block_size) as usize;
        let trace = self.trace();
        let reads = FuturesUnordered::new();
        while let Some(ItemRef {
            key:
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(attr_id, AttributeKey::Extent(extent_key)),
                },
            value: ObjectValue::Extent(extent_value),
            ..
        }) = iter.get()
        {
            if *object_id != self.object_id() || *attr_id != attribute_id {
                break;
            }
            ensure!(
                extent_key.is_valid() && block_size.is_aligned(extent_key),
                FxfsError::Inconsistent
            );
            if extent_key.start > offset {
                // Zero everything up to the start of the extent.
                let to_zero = min(extent_key.start - offset, buf.len() as u64) as usize;
                let len = buf.len();
                buf.reborrow().subslice_mut(0..to_zero).fill(0);
                buf = buf.subslice_mut(to_zero..len);
                if buf.is_empty() {
                    break;
                }
                offset += to_zero as u64;
            }

            if let ExtentValue::Some { device_offset, key_id, mode } = extent_value {
                let mut device_offset = device_offset + (offset - extent_key.start);
                let key_id = *key_id;

                let to_copy = min(buf.len() - end_align, (extent_key.end - offset) as usize);
                if to_copy > 0 {
                    if trace {
                        info!(
                            store_id = self.store().store_object_id(),
                            oid = self.object_id(),
                            device_range:? = (device_offset..device_offset + to_copy as u64),
                            offset,
                            range:? = **extent_key,
                            block_size = block_size.get();
                            "R",
                        );
                    }
                    let (mut head, tail) = buf.split_at_mut(to_copy);
                    let maybe_bitmap = match mode {
                        ExtentMode::OverwritePartial(bitmap) => {
                            let mut read_bitmap = bitmap
                                .clone()
                                .split_off(((offset - extent_key.start) / block_size) as usize);
                            read_bitmap.truncate(((to_copy as u64) / block_size) as usize);
                            Some(read_bitmap)
                        }
                        _ => None,
                    };
                    reads.push(async move {
                        self.read_and_decrypt(
                            attribute_id,
                            device_offset,
                            offset,
                            head.reborrow(),
                            key_id,
                        )
                        .await?;
                        if let Some(bitmap) = maybe_bitmap {
                            apply_bitmap_zeroing(self.block_size(), &bitmap, head);
                        }
                        Ok::<(), Error>(())
                    });
                    buf = tail;
                    if buf.is_empty() {
                        break;
                    }
                    offset += to_copy as u64;
                    device_offset += to_copy as u64;
                }

                // Deal with end alignment by reading the existing contents into an alignment
                // buffer.
                if offset < extent_key.end && end_align > 0 {
                    if let ExtentMode::OverwritePartial(bitmap) = mode {
                        let bitmap_offset = (offset - extent_key.start) / block_size;
                        if !bitmap.get(bitmap_offset as usize).ok_or(FxfsError::Inconsistent)? {
                            // If this block isn't actually initialized, skip it.
                            break;
                        }
                    }
                    let mut align_buf =
                        self.store().device.allocate_buffer(block_size.get() as usize).await;
                    if trace {
                        info!(
                            store_id = self.store().store_object_id(),
                            oid = self.object_id(),
                            device_range:? = (device_offset..device_offset + align_buf.len() as u64);
                            "RT",
                        );
                    }
                    self.read_and_decrypt(
                        attribute_id,
                        device_offset,
                        offset,
                        align_buf.as_mut(),
                        key_id,
                    )
                    .await?;
                    buf.copy_from_buffer(align_buf.as_ref().subslice(0..end_align));
                    buf = buf.subslice_mut(0..0);
                    break;
                }
            } else if extent_key.end >= offset + buf.len() as u64 {
                // Deleted extent covers remainder, so we're done.
                break;
            }

            iter.advance().await?;
        }
        reads.try_collect::<()>().await?;
        buf.fill(0);
        Ok(())
    }

    /// Reads an entire attribute.
    pub async fn read_attr(&self, attribute_id: AttributeId) -> Result<Option<Box<[u8]>>, Error> {
        let store = self.store();
        let tree = &store.tree;
        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let key = ObjectKey::attribute(self.object_id(), attribute_id, AttributeKey::Attribute);
        let iter = merger.query(Query::FullRange(&key)).await?;
        match iter.get() {
            Some(item) if item.key == &key => match item.value {
                ObjectValue::Attribute { .. } => Ok(Some(self.read_attr_from_iter(iter).await?)),
                // Attribute was deleted.
                ObjectValue::None => Ok(None),
                _ => Err(FxfsError::Inconsistent.into()),
            },
            _ => Ok(None),
        }
    }

    /// Reads an entire attribute pointed to by `iter`. `iter` must be pointing to the
    /// `AttributeKey::Attribute` of the attribute.
    pub async fn read_attr_from_iter(
        &self,
        mut iter: MergerIterator<'_, '_, ObjectKey, ObjectValue>,
    ) -> Result<Box<[u8]>, Error> {
        let (mut buffer, size, attribute_id) = match iter.get() {
            Some(ItemRef {
                key:
                    ObjectKey {
                        object_id,
                        data: ObjectKeyData::Attribute(attribute_id, AttributeKey::Attribute),
                    },
                value: ObjectValue::Attribute { size, .. },
                ..
            }) if *object_id == self.object_id => {
                // TODO(https://fxbug.dev/42073113): size > max buffer size
                (
                    self.store()
                        .device
                        .allocate_buffer(self.block_size().align_up(*size).unwrap() as usize)
                        .await,
                    *size as usize,
                    *attribute_id,
                )
            }
            _ => bail!(FxfsError::InvalidArgs),
        };

        self.store().logical_read_ops.fetch_add(1, Ordering::Relaxed);
        let mut last_offset = 0;
        loop {
            iter.advance().await?;
            match iter.get() {
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data:
                                ObjectKeyData::Attribute(attr_id, AttributeKey::Extent(extent_key)),
                        },
                    value: ObjectValue::Extent(extent_value),
                    ..
                }) if *object_id == self.object_id() && *attr_id == attribute_id => {
                    if let ExtentValue::Some { device_offset, key_id, mode } = extent_value {
                        let offset = extent_key.start as usize;
                        buffer.subslice_mut(last_offset..offset).fill(0);
                        let end = std::cmp::min(extent_key.end as usize, buffer.len());
                        let maybe_bitmap = match mode {
                            ExtentMode::OverwritePartial(bitmap) => {
                                // The caller has to adjust the bitmap if necessary, but we always
                                // start from the beginning of any extent, so we only truncate.
                                let mut read_bitmap = bitmap.clone();
                                read_bitmap.truncate(
                                    ((end as u64 - extent_key.start) / self.block_size()) as usize,
                                );
                                Some(read_bitmap)
                            }
                            _ => None,
                        };
                        self.read_and_decrypt(
                            attribute_id,
                            *device_offset,
                            extent_key.start,
                            buffer.subslice_mut(offset..end as usize),
                            *key_id,
                        )
                        .await?;
                        if let Some(bitmap) = maybe_bitmap {
                            apply_bitmap_zeroing(
                                self.block_size(),
                                &bitmap,
                                buffer.subslice_mut(offset..end as usize),
                            );
                        }
                        last_offset = end;
                        if last_offset >= size {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        buffer.subslice_mut(std::cmp::min(last_offset, size)..buffer.len()).fill(0);
        Ok(buffer.as_ref().subslice(0..size).to_vec().into_boxed_slice())
    }

    /// Writes potentially unaligned data at `device_offset` and returns checksums if requested.
    /// The data will be encrypted if necessary.  `buf` is mutable as an optimization, since the
    /// write may require encryption, we can encrypt the buffer in-place rather than copying to
    /// another buffer if the write is already aligned.
    ///
    /// NOTE: This will not create keys if they are missing (it will fail with an error if that
    /// happens to be the case).
    pub async fn write_at(
        &self,
        attribute_id: AttributeId,
        offset: u64,
        buf: MutableBufferRef<'_>,
        key_id: Option<u64>,
        device_offset: u64,
    ) -> Result<MaybeChecksums, Error> {
        self.write_at_with_flags(
            attribute_id,
            offset,
            buf,
            key_id,
            device_offset,
            WriteFlags::empty(),
        )
        .await
    }

    /// Same as `write_at`, but allows passing `WriteFlags` (such as `WriteFlags::PRE_BARRIER`) to
    /// the underlying device write.
    pub async fn write_at_with_flags(
        &self,
        attribute_id: AttributeId,
        offset: u64,
        buf: MutableBufferRef<'_>,
        key_id: Option<u64>,
        mut device_offset: u64,
        flags: WriteFlags,
    ) -> Result<MaybeChecksums, Error> {
        let mut transfer_buf;
        let block_size = self.block_size();
        let (range, mut transfer_buf_ref) =
            if offset % block_size == 0 && buf.len() as u64 % block_size == 0 {
                (offset..offset + buf.len() as u64, buf)
            } else {
                let (range, buf) = self.align_buffer(attribute_id, offset, buf.as_ref()).await?;
                transfer_buf = buf;
                device_offset -= offset - range.start;
                (range, transfer_buf.as_mut())
            };

        let mut crypt_ctx = None;
        if let (_, Some(key)) = self.get_key(key_id).await? {
            if let Some(ctx) = key.crypt_ctx(self.object_id, attribute_id.raw(), range.start) {
                crypt_ctx = Some(ctx);
            } else {
                key.encrypt(
                    self.object_id,
                    attribute_id.raw(),
                    device_offset,
                    range.start,
                    transfer_buf_ref.as_mut_ptr_slice(),
                )?;
            }
        }
        self.write_aligned(transfer_buf_ref.as_ref(), device_offset, crypt_ctx, flags).await
    }

    /// Writes to multiple ranges with data provided in `buf`. This function is specifically
    /// designed for migration purposes, allowing raw writes to the device without updating
    /// object metadata like allocated size or mtime. It's essential for scenarios where
    /// data needs to be transferred directly without triggering standard filesystem operations.
    #[cfg(feature = "migration")]
    pub async fn raw_multi_write(
        &self,
        transaction: &mut Transaction<'_>,
        attribute_id: AttributeId,
        key_id: Option<u64>,
        ranges: &[Range<u64>],
        buf: MutableBufferRef<'_>,
    ) -> Result<(), Error> {
        self.multi_write_internal(transaction, attribute_id, key_id, ranges, buf).await?;
        Ok(())
    }

    /// This is a low-level write function that writes to multiple ranges. Users should generally
    /// use `multi_write` instead of this function as this does not update the object's allocated
    /// size, mtime, atime, etc.
    ///
    /// Returns (allocated, deallocated) bytes on success.
    async fn multi_write_internal(
        &self,
        transaction: &mut Transaction<'_>,
        attribute_id: AttributeId,
        key_id: Option<u64>,
        ranges: &[Range<u64>],
        mut buf: MutableBufferRef<'_>,
    ) -> Result<(u64, u64), Error> {
        if buf.is_empty() {
            return Ok((0, 0));
        }
        let block_size = self.block_size();
        let store = self.store();
        let store_id = store.store_object_id();

        // The only key we allow to be created on-the-fly is a non permanent key wrapped with the
        // volume data key.
        let (key_id, key) = if key_id == Some(VOLUME_DATA_KEY_ID)
            && matches!(self.encryption, Encryption::CachedKeys)
        {
            (
                VOLUME_DATA_KEY_ID,
                Some(
                    self.get_or_create_key(transaction)
                        .await
                        .context("get_or_create_key failed")?,
                ),
            )
        } else {
            self.get_key(key_id).await?
        };
        if let Some(key) = &key {
            if !key.supports_inline_encryption() {
                let mut slice = buf.as_mut_ptr_slice();
                for r in ranges {
                    let l = r.end - r.start;
                    let (head, tail) = slice.split_at_mut(l as usize);
                    key.encrypt(
                        self.object_id,
                        attribute_id.raw(),
                        0, /* TODO(https://fxbug.dev/421269588): plumb through device_offset. */
                        r.start,
                        MutPtrByteSlice::from(head),
                    )?;
                    slice = tail;
                }
            }
        }

        let mut allocated = 0;
        let allocator = store.allocator();
        let trace = self.trace();
        let mut writes = FuturesOrdered::new();

        let mut logical_ranges = ranges.iter();
        let mut current_range = logical_ranges.next().unwrap().clone();

        while !buf.is_empty() {
            let mut device_range = allocator
                .allocate(transaction, store_id, buf.len() as u64)
                .await
                .context("allocation failed")?;
            if trace {
                info!(
                    store_id,
                    oid = self.object_id(),
                    device_range:?,
                    len = device_range.end - device_range.start;
                    "A",
                );
            }
            let mut device_range_len = device_range.end - device_range.start;
            allocated += device_range_len;
            // If inline encryption is NOT supported, this loop should only happen once.
            while device_range_len > 0 {
                if current_range.end <= current_range.start {
                    current_range = logical_ranges.next().unwrap().clone();
                }
                let (crypt_ctx, split) = if let Some(key) = &key {
                    if key.supports_inline_encryption() {
                        let split = std::cmp::min(
                            current_range.end - current_range.start,
                            device_range_len,
                        );
                        let crypt_ctx =
                            key.crypt_ctx(self.object_id, attribute_id.raw(), current_range.start);
                        current_range.start += split;
                        (crypt_ctx, split)
                    } else {
                        (None, device_range_len)
                    }
                } else {
                    (None, device_range_len)
                };

                let (head, tail) = buf.split_at_mut(split as usize);
                buf = tail;

                writes.push_back(async move {
                    let len = head.len() as u64;
                    Result::<_, Error>::Ok((
                        device_range.start,
                        len,
                        self.write_aligned(
                            head.as_ref(),
                            device_range.start,
                            crypt_ctx,
                            WriteFlags::empty(),
                        )
                        .await?,
                    ))
                });
                device_range.start += split;
                device_range_len = device_range.end - device_range.start;
            }
        }

        self.store().logical_write_ops.fetch_add(1, Ordering::Relaxed);
        let ((mutations, checksums), deallocated) = try_join!(
            async {
                let mut current_range = 0..0;
                let mut mutations = Vec::new();
                let mut out_checksums = Vec::new();
                let mut ranges = ranges.iter();
                while let Some((mut device_offset, mut len, mut checksums)) =
                    writes.try_next().await?
                {
                    while len > 0 {
                        if current_range.end <= current_range.start {
                            current_range = ranges.next().unwrap().clone();
                        }
                        let chunk_len = std::cmp::min(len, current_range.end - current_range.start);
                        let tail = checksums.split_off((chunk_len / block_size) as usize);
                        if let Some(checksums) = checksums.maybe_as_ref() {
                            out_checksums.push((
                                device_offset..device_offset + chunk_len,
                                checksums.to_owned(),
                            ));
                        }
                        mutations.push(Mutation::merge_object(
                            ObjectKey::extent(
                                self.object_id(),
                                attribute_id,
                                current_range.start..current_range.start + chunk_len,
                            ),
                            ObjectValue::Extent(ExtentValue::new(
                                device_offset,
                                checksums.to_mode(),
                                key_id,
                            )),
                        ));
                        checksums = tail;
                        device_offset += chunk_len;
                        len -= chunk_len;
                        current_range.start += chunk_len;
                    }
                }
                Result::<_, Error>::Ok((mutations, out_checksums))
            },
            async {
                let mut deallocated = 0;
                for r in ranges {
                    deallocated +=
                        self.deallocate_old_extents(transaction, attribute_id, r.clone()).await?;
                }
                Result::<_, Error>::Ok(deallocated)
            }
        )?;

        for m in mutations {
            transaction.add(store_id, m);
        }

        // Only store checksums in the journal if barriers are not enabled.
        if !store.filesystem().options().barriers_enabled {
            for (r, c) in checksums {
                transaction.add_checksum(r, c, true);
            }
        }
        Ok((allocated, deallocated))
    }

    /// Writes to multiple ranges with data provided in `buf`.  The buffer can be modified in place
    /// if encryption takes place.  The ranges must all be aligned and no change to content size is
    /// applied; the caller is responsible for updating size if required.  If `key_id` is None, it
    /// means pick the default key for the object which is the fscrypt key if present, or the volume
    /// data key, or no key if it's an unencrypted file.
    pub async fn multi_write(
        &self,
        transaction: &mut Transaction<'_>,
        attribute_id: AttributeId,
        key_id: Option<u64>,
        ranges: &[Range<u64>],
        buf: MutableBufferRef<'_>,
    ) -> Result<(), Error> {
        let (allocated, deallocated) =
            self.multi_write_internal(transaction, attribute_id, key_id, ranges, buf).await?;
        if allocated == 0 && deallocated == 0 {
            return Ok(());
        }
        self.update_allocated_size(transaction, allocated, deallocated).await
    }

    /// Write data to overwrite extents with the provided set of ranges. This makes a strong
    /// assumption that the ranges are actually going to be already allocated overwrite extents and
    /// will error out or do something wrong if they aren't. It also assumes the ranges passed to
    /// it are sorted.
    pub async fn multi_overwrite<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        attr_id: AttributeId,
        ranges: &[Range<u64>],
        mut buf: MutableBufferRef<'_>,
    ) -> Result<(), Error> {
        if buf.is_empty() {
            return Ok(());
        }
        let block_size = self.block_size();
        let store = self.store();
        let tree = store.tree();
        let store_id = store.store_object_id();

        let (key_id, key) = self.get_key(None).await?;
        if let Some(key) = &key {
            if !key.supports_inline_encryption() {
                let mut slice = buf.as_mut_ptr_slice();
                for r in ranges {
                    let l = r.end - r.start;
                    let (head, tail) = slice.split_at_mut(l as usize);
                    key.encrypt(
                        self.object_id,
                        attr_id.raw(),
                        0, /* TODO(https://fxbug.dev/421269588): plumb through device_offset. */
                        r.start,
                        MutPtrByteSlice::from(head),
                    )?;
                    slice = tail;
                }
            }
        }

        let mut range_iter = ranges.iter();
        // There should be at least one range if the buffer has data in it
        let mut target_range = range_iter.next().unwrap().clone();
        let mut mutations = Vec::new();
        let writes = FuturesUnordered::new();

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::attribute(
                self.object_id(),
                attr_id,
                AttributeKey::Extent(Extent::search_key_from_offset(target_range.start)),
            )))
            .await?;

        loop {
            match iter.get() {
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data:
                                ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent)),
                        },
                    value: ObjectValue::Extent(extent_value),
                    ..
                }) if *object_id == self.object_id() && *attribute_id == attr_id => {
                    // If this extent ends before the target range starts (not possible on the
                    // first loop because of the query parameters but possible on further loops),
                    // advance until we find a the next one we care about.
                    if extent.end <= target_range.start {
                        iter.advance().await?;
                        continue;
                    }
                    let (device_offset, mode) = match extent_value {
                        ExtentValue::None => {
                            return Err(anyhow!(FxfsError::Inconsistent)).with_context(|| {
                                format!(
                                    "multi_overwrite failed: target_range ({}, {}) overlaps with \
                                deleted extent found at ({}, {})",
                                    target_range.start, target_range.end, extent.start, extent.end,
                                )
                            });
                        }
                        ExtentValue::Some { device_offset, mode, .. } => (device_offset, mode),
                    };
                    // The ranges passed to this function should already by allocated, so
                    // extent records should exist for them.
                    if extent.start > target_range.start {
                        return Err(anyhow!(FxfsError::Inconsistent)).with_context(|| {
                            format!(
                                "multi_overwrite failed: target range ({}, {}) starts before first \
                            extent found at ({}, {})",
                                target_range.start, target_range.end, extent.start, extent.end,
                            )
                        });
                    }
                    let mut bitmap = match mode {
                        ExtentMode::Raw | ExtentMode::Cow(_) => {
                            return Err(anyhow!(FxfsError::Inconsistent)).with_context(|| {
                                format!(
                                    "multi_overwrite failed: \
                            extent from ({}, {}) which overlaps target range ({}, {}) had the \
                            wrong extent mode",
                                    extent.start, extent.end, target_range.start, target_range.end,
                                )
                            });
                        }
                        ExtentMode::OverwritePartial(bitmap) => {
                            OverwriteBitmaps::new(bitmap.clone())
                        }
                        ExtentMode::Overwrite => OverwriteBitmaps::None,
                    };
                    loop {
                        let offset_within_extent = target_range.start - extent.start;
                        let bitmap_offset = offset_within_extent / block_size;
                        let write_device_offset = *device_offset + offset_within_extent;
                        let write_end = min(extent.end, target_range.end);
                        let write_len = write_end - target_range.start;
                        let write_device_range =
                            write_device_offset..write_device_offset + write_len;
                        let (current_buf, remaining_buf) = buf.split_at_mut(write_len as usize);

                        bitmap.set_offset(bitmap_offset as usize);
                        let checksum_ranges = ChecksumRangeChunk::group_first_write_ranges(
                            &mut bitmap,
                            block_size,
                            write_device_range,
                        );

                        let crypt_ctx = if let Some(key) = &key {
                            key.crypt_ctx(self.object_id, attr_id.raw(), target_range.start)
                        } else {
                            None
                        };

                        writes.push(async move {
                            let maybe_checksums = self
                                .write_aligned(
                                    current_buf.as_ref(),
                                    write_device_offset,
                                    crypt_ctx,
                                    WriteFlags::empty(),
                                )
                                .await?;
                            Ok::<_, Error>(match maybe_checksums {
                                MaybeChecksums::None => Vec::new(),
                                MaybeChecksums::Fletcher(checksums) => checksum_ranges
                                    .into_iter()
                                    .map(
                                        |ChecksumRangeChunk {
                                             checksum_range,
                                             device_range,
                                             is_first_write,
                                         }| {
                                            (
                                                device_range,
                                                checksums[checksum_range].to_vec(),
                                                is_first_write,
                                            )
                                        },
                                    )
                                    .collect(),
                            })
                        });
                        buf = remaining_buf;
                        target_range.start += write_len;
                        if target_range.start == target_range.end {
                            match range_iter.next() {
                                None => break,
                                Some(next_range) => target_range = next_range.clone(),
                            }
                        }
                        if extent.end <= target_range.start {
                            break;
                        }
                    }
                    if let Some((mut bitmap, write_bitmap)) = bitmap.take_bitmaps() {
                        if bitmap.or(&write_bitmap) {
                            let mode = if bitmap.all() {
                                ExtentMode::Overwrite
                            } else {
                                ExtentMode::OverwritePartial(bitmap)
                            };
                            mutations.push(Mutation::merge_object(
                                ObjectKey::extent(self.object_id(), attr_id, extent.clone().into()),
                                ObjectValue::Extent(ExtentValue::new(*device_offset, mode, key_id)),
                            ))
                        }
                    }
                    if target_range.start == target_range.end {
                        break;
                    }
                    iter.advance().await?;
                }
                // We've either run past the end of the existing extents or something is wrong with
                // the tree. The main section should break if it finishes the ranges, so either
                // case, this is an error.
                _ => bail!(anyhow!(FxfsError::Internal).context(
                    "found a non-extent object record while there were still ranges to process"
                )),
            }
        }

        let checksums = writes.try_collect::<Vec<_>>().await?;
        // Only store checksums in the journal if barriers are not enabled.
        if !store.filesystem().options().barriers_enabled {
            for (r, c, first_write) in checksums.into_iter().flatten() {
                transaction.add_checksum(r, c, first_write);
            }
        }

        for m in mutations {
            transaction.add(store_id, m);
        }

        Ok(())
    }

    /// Writes an attribute that should not already exist and therefore does not require trimming.
    /// Breaks up the write into multiple transactions if `data.len()` is larger than `batch_size`.
    /// If writing the attribute requires multiple transactions, adds the attribute to the
    /// graveyard. The caller is responsible for removing the attribute from the graveyard when it
    /// commits the last transaction.  This always writes using a key wrapped with the volume data
    /// key.
    #[trace]
    pub async fn write_new_attr_in_batches<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        attribute_id: AttributeId,
        data: &[u8],
        batch_size: usize,
    ) -> Result<(), Error> {
        transaction.add(
            self.store().store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(self.object_id(), attribute_id, AttributeKey::Attribute),
                ObjectValue::attribute(data.len() as u64, false),
            ),
        );
        let chunks = data.chunks(batch_size);
        let num_chunks = chunks.len();
        if num_chunks > 1 {
            transaction.add(
                self.store().store_object_id,
                Mutation::replace_or_insert_object(
                    ObjectKey::graveyard_attribute_entry(
                        self.store().graveyard_directory_object_id(),
                        self.object_id(),
                        attribute_id,
                    ),
                    ObjectValue::Some,
                ),
            );
        }
        let mut start_offset = 0;
        for (i, chunk) in chunks.enumerate() {
            let rounded_len = self.block_size().align_up(chunk.len() as u64).unwrap();
            let mut buffer = self.store().device.allocate_buffer(rounded_len as usize).await;
            let mut slice = buffer.as_mut_ptr_slice();
            slice.subslice_mut(0..chunk.len()).copy_from_slice(chunk);
            slice.subslice_mut(chunk.len()..slice.len()).fill(0);
            self.multi_write(
                transaction,
                attribute_id,
                Some(VOLUME_DATA_KEY_ID),
                &[start_offset..start_offset + rounded_len],
                buffer.as_mut(),
            )
            .await?;
            start_offset += rounded_len;
            // Do not commit the last chunk.
            if i < num_chunks - 1 {
                transaction.commit_and_continue().await?;
            }
        }
        Ok(())
    }

    /// Writes an entire attribute. Returns whether or not the attribute needs to continue being
    /// trimmed - if the new data is shorter than the old data, this will trim any extents beyond
    /// the end of the new size, but if there were too many for a single transaction, a commit
    /// needs to be made before trimming again, so the responsibility is left to the caller so as
    /// to not accidentally split the transaction when it's not in a consistent state.  This will
    /// write using the volume data key; the fscrypt key is not supported.
    pub async fn write_attr(
        &self,
        transaction: &mut Transaction<'_>,
        attribute_id: AttributeId,
        data: &[u8],
    ) -> Result<NeedsTrim, Error> {
        let rounded_len = self.block_size().align_up(data.len() as u64).unwrap();
        let store = self.store();
        let tree = store.tree();
        let should_trim = tree
            .find_map(
                &ObjectKey::attribute(self.object_id(), attribute_id, AttributeKey::Attribute),
                |item| match item.value {
                    ObjectValue::Attribute { size: _, has_overwrite_extents: true } => {
                        Err(anyhow!(FxfsError::Inconsistent)
                            .context("write_attr on an attribute with overwrite extents"))
                    }
                    ObjectValue::Attribute { size, .. } => Ok((data.len() as u64) < *size),
                    _ => Err(FxfsError::Inconsistent.into()),
                },
            )
            .await?
            .transpose()?
            .unwrap_or(false);
        let mut buffer = self.store().device.allocate_buffer(rounded_len as usize).await;
        let mut slice = buffer.as_mut_ptr_slice();
        slice.subslice_mut(0..data.len()).copy_from_slice(data);
        slice.subslice_mut(data.len()..slice.len()).fill(0);
        self.multi_write(
            transaction,
            attribute_id,
            Some(VOLUME_DATA_KEY_ID),
            &[0..rounded_len],
            buffer.as_mut(),
        )
        .await?;
        transaction.add(
            self.store().store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(self.object_id(), attribute_id, AttributeKey::Attribute),
                ObjectValue::attribute(data.len() as u64, false),
            ),
        );
        if should_trim {
            self.shrink(transaction, attribute_id, data.len() as u64).await
        } else {
            Ok(NeedsTrim(false))
        }
    }

    pub async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, Error> {
        let layer_set = self.store().tree().layer_set();
        let mut merger = layer_set.merger();
        // Seek to the first extended attribute key for this object.
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::extended_attribute(self.object_id(), Vec::new())))
            .await?;
        let mut out = Vec::new();
        while let Some(item) = iter.get() {
            // Skip deleted extended attributes.
            if item.value != &ObjectValue::None {
                match item.key {
                    ObjectKey { object_id, data: ObjectKeyData::ExtendedAttribute { name } }
                        if *object_id == self.object_id() =>
                    {
                        out.push(name.clone());
                    }
                    // Once we hit a record belonging to another object, or one that is not an
                    // extended attribute key, we have reached the end of this object's extended
                    // attributes. Subsequent objects' records will start with lower variants
                    // (e.g. ObjectKeyData::Object) which trigger this break.
                    _ => break,
                }
            }
            iter.advance().await?;
        }
        Ok(out)
    }

    /// Looks up the values for the extended attribute `fio::SELINUX_CONTEXT_NAME`, returning it
    /// if it is found inline. If it is not inline, it will request use of the
    /// `get_extended_attributes` method. If the entry doesn't exist at all, returns None.
    pub async fn get_inline_selinux_context(&self) -> Result<Option<fio::SelinuxContext>, Error> {
        // This optimization is only useful as long as the attribute is smaller than inline sizes.
        // Avoid reading the data out of the attributes.
        const_assert!(fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize <= MAX_INLINE_XATTR_SIZE);
        self.store()
            .tree()
            .find_map(
                &ObjectKey::extended_attribute(self.object_id(), fio::SELINUX_CONTEXT_NAME.into()),
                |item| match item.value {
                    ObjectValue::ExtendedAttribute(ExtendedAttributeValue::Inline(value)) => {
                        Ok(fio::SelinuxContext::Data(value.clone()))
                    }
                    ObjectValue::ExtendedAttribute(ExtendedAttributeValue::AttributeId(_)) => {
                        Ok(fio::SelinuxContext::UseExtendedAttributes(fio::EmptyStruct {}))
                    }
                    _ => Err(anyhow!(FxfsError::Inconsistent).context(
                        "get_inline_extended_attribute: Expected ExtendedAttribute value",
                    )),
                },
            )
            .await?
            .transpose()
    }

    pub async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, Error> {
        let value = self
            .store()
            .tree()
            .find_value(&ObjectKey::extended_attribute(self.object_id(), name))
            .await?
            .ok_or(FxfsError::NotFound)?;
        match value {
            ObjectValue::ExtendedAttribute(ExtendedAttributeValue::Inline(value)) => Ok(value),
            ObjectValue::ExtendedAttribute(ExtendedAttributeValue::AttributeId(id)) => {
                ensure!(id.is_xattr(), FxfsError::Inconsistent);
                Ok(self.read_attr(id).await?.ok_or(FxfsError::Inconsistent)?.into_vec())
            }
            _ => {
                bail!(
                    anyhow!(FxfsError::Inconsistent)
                        .context("get_extended_attribute: Expected ExtendedAttribute value")
                )
            }
        }
    }

    pub async fn set_extended_attribute(
        &self,
        name: Vec<u8>,
        value: Vec<u8>,
        mode: SetExtendedAttributeMode,
    ) -> Result<(), Error> {
        let store = self.store();
        // NB: We need to take this lock before we potentially look up the value to prevent racing
        // with another set.
        let keys = lock_keys![LockKey::object(store.store_object_id(), self.object_id())];
        let mut transaction = store.new_transaction(keys, Options::default()).await?;
        self.set_extended_attribute_impl(name, value, mode, &mut transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn set_extended_attribute_impl(
        &self,
        name: Vec<u8>,
        value: Vec<u8>,
        mode: SetExtendedAttributeMode,
        transaction: &mut Transaction<'_>,
    ) -> Result<(), Error> {
        ensure!(name.len() <= MAX_XATTR_NAME_SIZE, FxfsError::TooBig);
        ensure!(value.len() <= MAX_XATTR_VALUE_SIZE, FxfsError::TooBig);
        let tree = self.store().tree();
        let object_key = ObjectKey::extended_attribute(self.object_id(), name);

        let existing_attribute_id = {
            let find_result = tree
                .find_map(&object_key, |item| match item.value {
                    ObjectValue::ExtendedAttribute(ExtendedAttributeValue::Inline(..)) => Ok(None),
                    ObjectValue::ExtendedAttribute(ExtendedAttributeValue::AttributeId(id)) => {
                        ensure!(id.is_xattr(), FxfsError::Inconsistent);
                        Ok(Some(*id))
                    }
                    _ => Err(anyhow!(FxfsError::Inconsistent)
                        .context("expected extended attribute value")),
                })
                .await?;
            let (found, existing_attribute_id) = match find_result {
                Some(id) => (true, id?),
                None => (false, None),
            };
            match mode {
                SetExtendedAttributeMode::Create if found => {
                    bail!(FxfsError::AlreadyExists)
                }
                SetExtendedAttributeMode::Replace if !found => {
                    bail!(FxfsError::NotFound)
                }
                _ => (),
            }
            existing_attribute_id
        };

        if let Some(attribute_id) = existing_attribute_id {
            // If we already have an attribute id allocated for this extended attribute, we always
            // use it, even if the value has shrunk enough to be stored inline. We don't need to
            // worry about trimming here for the same reason we don't need to worry about it when
            // we delete xattrs - they simply aren't large enough to ever need more than one
            // transaction.
            let _ = self.write_attr(transaction, attribute_id, &value).await?;
        } else if value.len() <= MAX_INLINE_XATTR_SIZE {
            transaction.add(
                self.store().store_object_id(),
                Mutation::replace_or_insert_object(
                    object_key,
                    ObjectValue::inline_extended_attribute(value),
                ),
            );
        } else {
            // If there isn't an existing attribute id and we are going to store the value in
            // an attribute, find the next empty attribute id in the range. We search for fxfs
            // attribute records specifically, instead of the extended attribute records, because
            // even if the extended attribute record is removed the attribute may not be fully
            // trimmed yet.
            let mut attribute_id = AttributeId::XATTR_RANGE_START;
            let layer_set = tree.layer_set();
            let mut merger = layer_set.merger();
            let key = ObjectKey::attribute(self.object_id(), attribute_id, AttributeKey::Attribute);
            let mut iter = merger.query(Query::FullRange(&key)).await?;
            loop {
                match iter.get() {
                    // None means the key passed to seek wasn't found. That means the first
                    // attribute is available and we can just stop right away.
                    None => break,
                    Some(ItemRef {
                        key: ObjectKey { object_id, data: ObjectKeyData::Attribute(attr_id, _) },
                        value,
                        ..
                    }) if *object_id == self.object_id() => {
                        if matches!(value, ObjectValue::None) {
                            // This attribute was once used but is now deleted, so it's safe to use
                            // again.
                            break;
                        }
                        if attribute_id < *attr_id {
                            // We found a gap - use it.
                            break;
                        } else if attribute_id == *attr_id {
                            // This attribute id is in use, try the next one.
                            attribute_id = attribute_id.next();
                            if attribute_id == AttributeId::XATTR_RANGE_END {
                                bail!(FxfsError::NoSpace);
                            }
                        }
                        // If we don't hit either of those cases, we are still moving through the
                        // extent keys for the current attribute, so just keep advancing until the
                        // attribute id changes.
                    }
                    // As we are working our way through the iterator, if we hit anything that
                    // doesn't have our object id or attribute key data, we've gone past the end of
                    // this section and can stop.
                    _ => break,
                }
                iter.advance().await?;
            }

            // We know this won't need trimming because it's a new attribute.
            let _ = self.write_attr(transaction, attribute_id, &value).await?;
            transaction.add(
                self.store().store_object_id(),
                Mutation::replace_or_insert_object(
                    object_key,
                    ObjectValue::extended_attribute(attribute_id),
                ),
            );
        }

        Ok(())
    }

    pub async fn remove_extended_attribute(&self, name: Vec<u8>) -> Result<(), Error> {
        let store = self.store();
        let tree = store.tree();
        let object_key = ObjectKey::extended_attribute(self.object_id(), name);

        // NB: The API says we have to return an error if the attribute doesn't exist, so we have
        // to look it up first to make sure we have a record of it before we delete it. Make sure
        // we take a lock and make a transaction before we do so we don't race with other
        // operations.
        let keys = lock_keys![LockKey::object(store.store_object_id(), self.object_id())];
        let mut transaction = store.new_transaction(keys, Options::default()).await?;

        let attribute_to_delete = tree
            .find_map(&object_key, |item| match item.value {
                ObjectValue::ExtendedAttribute(ExtendedAttributeValue::AttributeId(id)) => {
                    ensure!(id.is_xattr(), FxfsError::Inconsistent);
                    Ok(Some(*id))
                }
                ObjectValue::ExtendedAttribute(ExtendedAttributeValue::Inline(..)) => Ok(None),
                _ => Err(anyhow!(FxfsError::Inconsistent)
                    .context("remove_extended_attribute: Expected ExtendedAttribute value")),
            })
            .await?
            .ok_or(FxfsError::NotFound)??;

        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(object_key, ObjectValue::None),
        );

        // If the attribute wasn't stored inline, we need to deallocate all the extents too. This
        // would normally need to interact with the graveyard for correctness - if there are too
        // many extents to delete to fit in a single transaction then we could potentially have
        // consistency issues. However, the maximum size of an extended attribute is small enough
        // that it will never come close to that limit even in the worst case, so we just delete
        // everything in one shot.
        if let Some(attribute_id) = attribute_to_delete {
            let trim_result = store
                .trim_some(
                    &mut transaction,
                    self.object_id(),
                    attribute_id,
                    TrimMode::FromOffset(0),
                )
                .await?;
            // In case you didn't read the comment above - this should not be used to delete
            // arbitrary attributes!
            assert_matches!(trim_result, TrimResult::Done(_));
            transaction.add(
                store.store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::attribute(self.object_id, attribute_id, AttributeKey::Attribute),
                    ObjectValue::None,
                ),
            );
        }

        transaction.commit().await?;
        Ok(())
    }

    /// Returns a future that will pre-fetches the keys so as to avoid paying the performance
    /// penalty later. Must ensure that the object is not removed before the future completes.
    pub fn pre_fetch_keys(&self) -> Option<impl Future<Output = ()> + use<S>> {
        if let Encryption::CachedKeys = self.encryption {
            let owner = self.owner.clone();
            let object_id = self.object_id;
            Some(async move {
                let store = owner.as_ref().as_ref();
                if let Some(crypt) = store.crypt() {
                    let _ = store
                        .key_manager
                        .get_keys(
                            object_id,
                            crypt.as_ref(),
                            &mut Some(async || store.get_keys(object_id).await),
                            /* permanent= */ false,
                            /* force= */ false,
                        )
                        .await;
                }
            })
        } else {
            None
        }
    }
}

impl<S: HandleOwner> Drop for StoreObjectHandle<S> {
    fn drop(&mut self) {
        if self.is_encrypted() {
            let _ = self.store().key_manager.remove(self.object_id);
        }
    }
}

/// When truncating an object, sometimes it might not be possible to complete the transaction in a
/// single transaction, in which case the caller needs to finish trimming the object in subsequent
/// transactions (by calling ObjectStore::trim).
#[must_use]
pub struct NeedsTrim(pub bool);

#[cfg(test)]
mod tests {
    use super::{ChecksumRangeChunk, OverwriteBitmaps};
    use crate::errors::FxfsError;
    use crate::filesystem::{FxFilesystem, OpenFxFilesystem};
    use crate::object_handle::{ObjectHandle, WriteObjectHandle};
    use crate::object_store::data_object_handle::WRITE_ATTR_BATCH_SIZE;
    use crate::object_store::transaction::{Mutation, Options, lock_keys};
    use crate::object_store::{
        AttributeId, AttributeKey, DataObjectHandle, Directory, HandleOptions, LockKey, ObjectKey,
        ObjectStore, ObjectValue, SetExtendedAttributeMode, StoreObjectHandle,
    };
    use bit_vec::BitVec;
    use fuchsia_async as fasync;
    use futures::{TryStreamExt, join};
    use std::sync::Arc;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;
    use storage_units::BlockSize;

    const TEST_DEVICE_BLOCK_SIZE: u32 = 512;
    const TEST_OBJECT_NAME: &str = "foo";

    fn is_error(actual: anyhow::Error, expected: FxfsError) {
        assert_eq!(*actual.root_cause().downcast_ref::<FxfsError>().unwrap(), expected)
    }

    async fn test_filesystem() -> OpenFxFilesystem {
        let device = DeviceHolder::new(FakeDevice::new(16384, TEST_DEVICE_BLOCK_SIZE));
        FxFilesystem::new_empty(device).await.expect("new_empty failed")
    }

    async fn test_filesystem_and_empty_object() -> (OpenFxFilesystem, DataObjectHandle<ObjectStore>)
    {
        let fs = test_filesystem().await;
        let store = fs.root_store();

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        let object =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");

        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        root_directory
            .add_child_file(&mut transaction, TEST_OBJECT_NAME, &object)
            .await
            .expect("add_child_file failed");

        transaction.commit().await.expect("commit failed");

        (fs, object)
    }

    #[fuchsia::test]
    async fn test_extent_stream() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        // Write 8KiB at offset 0 and 8KiB at offset 16KiB, leaving a hole in between.
        let mut buf = object.allocate_buffer(8192).await;
        buf.as_mut_ptr_slice().fill(0xaa);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        object.write_or_append(Some(16384), buf.as_ref()).await.expect("write failed");

        let basic = StoreObjectHandle::new(
            object.owner().clone(),
            object.object_id(),
            /* permanent_keys: */ false,
            HandleOptions::default(),
            false,
        );

        let store = fs.root_store();
        let layer_set = store.tree.layer_set();
        let mut merger = layer_set.merger();

        let stream = basic
            .extent_stream(&mut merger, AttributeId::DATA)
            .await
            .expect("extent_stream failed");

        let result: Vec<_> = stream.try_collect().await.expect("stream item error");

        // Expect 2 physical mappings
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].logical_offset(), 0);
        assert_eq!(result[0].length(), 8192);
        assert_eq!(result[1].logical_offset(), 16384);
        assert_eq!(result[1].length(), 8192);
    }

    #[fuchsia::test(threads = 3)]
    async fn extended_attribute_double_remove() {
        // This test is intended to trip a potential race condition in remove. Removing an
        // attribute that doesn't exist is an error, so we need to check before we remove, but if
        // we aren't careful, two parallel removes might both succeed in the check and then both
        // remove the value.
        let (fs, object) = test_filesystem_and_empty_object().await;
        let basic = Arc::new(StoreObjectHandle::new(
            object.owner().clone(),
            object.object_id(),
            /* permanent_keys: */ false,
            HandleOptions::default(),
            false,
        ));
        let basic_a = basic.clone();
        let basic_b = basic.clone();

        basic
            .set_extended_attribute(
                b"security.selinux".to_vec(),
                b"bar".to_vec(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .expect("failed to set attribute");

        // Try to remove the attribute twice at the same time. One should succeed in the race and
        // return Ok, and the other should fail the race and return NOT_FOUND.
        let a_task = fasync::Task::spawn(async move {
            basic_a.remove_extended_attribute(b"security.selinux".to_vec()).await
        });
        let b_task = fasync::Task::spawn(async move {
            basic_b.remove_extended_attribute(b"security.selinux".to_vec()).await
        });
        match join!(a_task, b_task) {
            (Ok(()), Ok(())) => panic!("both remove calls succeeded"),
            (Err(_), Err(_)) => panic!("both remove calls failed"),

            (Ok(()), Err(e)) => is_error(e, FxfsError::NotFound),
            (Err(e), Ok(())) => is_error(e, FxfsError::NotFound),
        }

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test(threads = 3)]
    async fn extended_attribute_double_create() {
        // This test is intended to trip a potential race in set when using the create flag,
        // similar to above. If the create mode is set, we need to check that the attribute isn't
        // already created, but if two parallel creates both succeed in that check, and we aren't
        // careful with locking, they will both succeed and one will overwrite the other.
        let (fs, object) = test_filesystem_and_empty_object().await;
        let basic = Arc::new(StoreObjectHandle::new(
            object.owner().clone(),
            object.object_id(),
            /* permanent_keys: */ false,
            HandleOptions::default(),
            false,
        ));
        let basic_a = basic.clone();
        let basic_b = basic.clone();

        // Try to set the attribute twice at the same time. One should succeed in the race and
        // return Ok, and the other should fail the race and return ALREADY_EXISTS.
        let a_task = fasync::Task::spawn(async move {
            basic_a
                .set_extended_attribute(
                    b"security.selinux".to_vec(),
                    b"one".to_vec(),
                    SetExtendedAttributeMode::Create,
                )
                .await
        });
        let b_task = fasync::Task::spawn(async move {
            basic_b
                .set_extended_attribute(
                    b"security.selinux".to_vec(),
                    b"two".to_vec(),
                    SetExtendedAttributeMode::Create,
                )
                .await
        });
        match join!(a_task, b_task) {
            (Ok(()), Ok(())) => panic!("both set calls succeeded"),
            (Err(_), Err(_)) => panic!("both set calls failed"),

            (Ok(()), Err(e)) => {
                assert_eq!(
                    basic
                        .get_extended_attribute(b"security.selinux".to_vec())
                        .await
                        .expect("failed to get xattr"),
                    b"one"
                );
                is_error(e, FxfsError::AlreadyExists);
            }
            (Err(e), Ok(())) => {
                assert_eq!(
                    basic
                        .get_extended_attribute(b"security.selinux".to_vec())
                        .await
                        .expect("failed to get xattr"),
                    b"two"
                );
                is_error(e, FxfsError::AlreadyExists);
            }
        }

        fs.close().await.expect("Close failed");
    }

    struct TestAttr {
        name: Vec<u8>,
        value: Vec<u8>,
    }

    impl TestAttr {
        fn new(name: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Self {
            Self { name: name.as_ref().to_vec(), value: value.as_ref().to_vec() }
        }
        fn name(&self) -> Vec<u8> {
            self.name.clone()
        }
        fn value(&self) -> Vec<u8> {
            self.value.clone()
        }
    }

    #[fuchsia::test]
    async fn extended_attributes() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let test_attr = TestAttr::new(b"security.selinux", b"foo");

        assert_eq!(object.list_extended_attributes().await.unwrap(), Vec::<Vec<u8>>::new());
        is_error(
            object.get_extended_attribute(test_attr.name()).await.unwrap_err(),
            FxfsError::NotFound,
        );

        object
            .set_extended_attribute(
                test_attr.name(),
                test_attr.value(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(object.list_extended_attributes().await.unwrap(), vec![test_attr.name()]);
        assert_eq!(
            object.get_extended_attribute(test_attr.name()).await.unwrap(),
            test_attr.value()
        );

        object.remove_extended_attribute(test_attr.name()).await.unwrap();
        assert_eq!(object.list_extended_attributes().await.unwrap(), Vec::<Vec<u8>>::new());
        is_error(
            object.get_extended_attribute(test_attr.name()).await.unwrap_err(),
            FxfsError::NotFound,
        );

        // Make sure we can object the same attribute being set again.
        object
            .set_extended_attribute(
                test_attr.name(),
                test_attr.value(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(object.list_extended_attributes().await.unwrap(), vec![test_attr.name()]);
        assert_eq!(
            object.get_extended_attribute(test_attr.name()).await.unwrap(),
            test_attr.value()
        );

        object.remove_extended_attribute(test_attr.name()).await.unwrap();
        assert_eq!(object.list_extended_attributes().await.unwrap(), Vec::<Vec<u8>>::new());
        is_error(
            object.get_extended_attribute(test_attr.name()).await.unwrap_err(),
            FxfsError::NotFound,
        );

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn extended_attribute_invalid_id_range() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let store = object.owner();
        let mut transaction = store
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), object.object_id())],
                Options::default(),
            )
            .await
            .unwrap();
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::extended_attribute(object.object_id(), b"bad_attr".to_vec()),
                ObjectValue::extended_attribute(AttributeId::DATA),
            ),
        );
        transaction.commit().await.unwrap();

        is_error(
            object.get_extended_attribute(b"bad_attr".to_vec()).await.unwrap_err(),
            FxfsError::Inconsistent,
        );
        is_error(
            object
                .set_extended_attribute(
                    b"bad_attr".to_vec(),
                    b"new_val".to_vec(),
                    SetExtendedAttributeMode::Set,
                )
                .await
                .unwrap_err(),
            FxfsError::Inconsistent,
        );
        is_error(
            object.remove_extended_attribute(b"bad_attr".to_vec()).await.unwrap_err(),
            FxfsError::Inconsistent,
        );

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn large_extended_attribute() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let test_attr = TestAttr::new(b"security.selinux", vec![3u8; 300]);

        object
            .set_extended_attribute(
                test_attr.name(),
                test_attr.value(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(
            object.get_extended_attribute(test_attr.name()).await.unwrap(),
            test_attr.value()
        );

        // Probe the fxfs attributes to make sure it did the expected thing. This relies on inside
        // knowledge of how the attribute id is chosen.
        assert_eq!(
            object
                .read_attr(AttributeId::XATTR_RANGE_START)
                .await
                .expect("read_attr failed")
                .expect("read_attr returned none")
                .into_vec(),
            test_attr.value()
        );

        object.remove_extended_attribute(test_attr.name()).await.unwrap();
        is_error(
            object.get_extended_attribute(test_attr.name()).await.unwrap_err(),
            FxfsError::NotFound,
        );

        // Make sure we can object the same attribute being set again.
        object
            .set_extended_attribute(
                test_attr.name(),
                test_attr.value(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(
            object.get_extended_attribute(test_attr.name()).await.unwrap(),
            test_attr.value()
        );
        object.remove_extended_attribute(test_attr.name()).await.unwrap();
        is_error(
            object.get_extended_attribute(test_attr.name()).await.unwrap_err(),
            FxfsError::NotFound,
        );

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn multiple_extended_attributes() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let attrs = [
            TestAttr::new(b"security.selinux", b"foo"),
            TestAttr::new(b"large.attribute", vec![3u8; 300]),
            TestAttr::new(b"an.attribute", b"asdf"),
            TestAttr::new(b"user.big", vec![5u8; 288]),
            TestAttr::new(b"user.tiny", b"smol"),
            TestAttr::new(b"this string doesn't matter", b"the quick brown fox etc"),
            TestAttr::new(b"also big", vec![7u8; 500]),
            TestAttr::new(b"all.ones", vec![1u8; 11111]),
        ];

        for i in 0..attrs.len() {
            object
                .set_extended_attribute(
                    attrs[i].name(),
                    attrs[i].value(),
                    SetExtendedAttributeMode::Set,
                )
                .await
                .unwrap();
            assert_eq!(
                object.get_extended_attribute(attrs[i].name()).await.unwrap(),
                attrs[i].value()
            );
        }

        for i in 0..attrs.len() {
            // Make sure expected attributes are still available.
            let mut found_attrs = object.list_extended_attributes().await.unwrap();
            let mut expected_attrs: Vec<Vec<u8>> = attrs.iter().skip(i).map(|a| a.name()).collect();
            found_attrs.sort();
            expected_attrs.sort();
            assert_eq!(found_attrs, expected_attrs);
            for j in i..attrs.len() {
                assert_eq!(
                    object.get_extended_attribute(attrs[j].name()).await.unwrap(),
                    attrs[j].value()
                );
            }

            object.remove_extended_attribute(attrs[i].name()).await.expect("failed to remove");
            is_error(
                object.get_extended_attribute(attrs[i].name()).await.unwrap_err(),
                FxfsError::NotFound,
            );
        }

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn multiple_extended_attributes_delete() {
        let (fs, object) = test_filesystem_and_empty_object().await;
        let store = object.owner().clone();

        let attrs = [
            TestAttr::new(b"security.selinux", b"foo"),
            TestAttr::new(b"large.attribute", vec![3u8; 300]),
            TestAttr::new(b"an.attribute", b"asdf"),
            TestAttr::new(b"user.big", vec![5u8; 288]),
            TestAttr::new(b"user.tiny", b"smol"),
            TestAttr::new(b"this string doesn't matter", b"the quick brown fox etc"),
            TestAttr::new(b"also big", vec![7u8; 500]),
            TestAttr::new(b"all.ones", vec![1u8; 11111]),
        ];

        for i in 0..attrs.len() {
            object
                .set_extended_attribute(
                    attrs[i].name(),
                    attrs[i].value(),
                    SetExtendedAttributeMode::Set,
                )
                .await
                .unwrap();
            assert_eq!(
                object.get_extended_attribute(attrs[i].name()).await.unwrap(),
                attrs[i].value()
            );
        }

        // Unlink the file
        let root_directory =
            Directory::open(object.owner(), object.store().root_directory_object_id())
                .await
                .expect("open failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), store.root_directory_object_id()),
                    LockKey::object(store.store_object_id(), object.object_id()),
                ],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        crate::object_store::directory::replace_child(
            &mut transaction,
            None,
            (&root_directory, TEST_OBJECT_NAME),
        )
        .await
        .expect("replace_child failed");
        transaction.commit().await.unwrap();
        store.tombstone_object(object.object_id(), Options::default(), None).await.unwrap();

        crate::fsck::fsck(fs.clone()).await.unwrap();

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn extended_attribute_changing_sizes() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let test_name = b"security.selinux";
        let test_small_attr = TestAttr::new(test_name, b"smol");
        let test_large_attr = TestAttr::new(test_name, vec![3u8; 300]);

        object
            .set_extended_attribute(
                test_small_attr.name(),
                test_small_attr.value(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(
            object.get_extended_attribute(test_small_attr.name()).await.unwrap(),
            test_small_attr.value()
        );

        // With a small attribute, we don't expect it to write to an fxfs attribute.
        assert!(
            object
                .read_attr(AttributeId::XATTR_RANGE_START)
                .await
                .expect("read_attr failed")
                .is_none()
        );

        crate::fsck::fsck(fs.clone()).await.unwrap();

        object
            .set_extended_attribute(
                test_large_attr.name(),
                test_large_attr.value(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(
            object.get_extended_attribute(test_large_attr.name()).await.unwrap(),
            test_large_attr.value()
        );

        // Once the value is above the threshold, we expect it to get upgraded to an fxfs
        // attribute.
        assert_eq!(
            object
                .read_attr(AttributeId::XATTR_RANGE_START)
                .await
                .expect("read_attr failed")
                .expect("read_attr returned none")
                .into_vec(),
            test_large_attr.value()
        );

        crate::fsck::fsck(fs.clone()).await.unwrap();

        object
            .set_extended_attribute(
                test_small_attr.name(),
                test_small_attr.value(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(
            object.get_extended_attribute(test_small_attr.name()).await.unwrap(),
            test_small_attr.value()
        );

        // Even though we are back under the threshold, we still expect it to be stored in an fxfs
        // attribute, because we don't downgrade to inline once we've allocated one.
        assert_eq!(
            object
                .read_attr(AttributeId::XATTR_RANGE_START)
                .await
                .expect("read_attr failed")
                .expect("read_attr returned none")
                .into_vec(),
            test_small_attr.value()
        );

        crate::fsck::fsck(fs.clone()).await.unwrap();

        object.remove_extended_attribute(test_small_attr.name()).await.expect("failed to remove");

        crate::fsck::fsck(fs.clone()).await.unwrap();

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn extended_attribute_max_size() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let test_attr = TestAttr::new(
            vec![3u8; super::MAX_XATTR_NAME_SIZE],
            vec![1u8; super::MAX_XATTR_VALUE_SIZE],
        );

        object
            .set_extended_attribute(
                test_attr.name(),
                test_attr.value(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();
        assert_eq!(
            object.get_extended_attribute(test_attr.name()).await.unwrap(),
            test_attr.value()
        );
        assert_eq!(object.list_extended_attributes().await.unwrap(), vec![test_attr.name()]);
        object.remove_extended_attribute(test_attr.name()).await.unwrap();

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn extended_attribute_remove_then_create() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let test_attr = TestAttr::new(
            vec![3u8; super::MAX_XATTR_NAME_SIZE],
            vec![1u8; super::MAX_XATTR_VALUE_SIZE],
        );

        object
            .set_extended_attribute(
                test_attr.name(),
                test_attr.value(),
                SetExtendedAttributeMode::Create,
            )
            .await
            .unwrap();
        fs.journal().force_compact().await.unwrap();
        object.remove_extended_attribute(test_attr.name()).await.unwrap();
        object
            .set_extended_attribute(
                test_attr.name(),
                test_attr.value(),
                SetExtendedAttributeMode::Create,
            )
            .await
            .unwrap();

        assert_eq!(
            object.get_extended_attribute(test_attr.name()).await.unwrap(),
            test_attr.value()
        );

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn large_extended_attribute_max_number() {
        let (fs, object) = test_filesystem_and_empty_object().await;

        let max_xattrs = AttributeId::XATTR_RANGE_END.raw() - AttributeId::XATTR_RANGE_START.raw();
        for i in 0..max_xattrs {
            let test_attr = TestAttr::new(format!("{}", i).as_bytes(), vec![0x3; 300]);
            object
                .set_extended_attribute(
                    test_attr.name(),
                    test_attr.value(),
                    SetExtendedAttributeMode::Set,
                )
                .await
                .unwrap_or_else(|_| panic!("failed to set xattr number {}", i));
        }

        // That should have taken up all the attributes we've allocated to extended attributes, so
        // this one should return ERR_NO_SPACE.
        match object
            .set_extended_attribute(
                b"one.too.many".to_vec(),
                vec![0x3; 300],
                SetExtendedAttributeMode::Set,
            )
            .await
        {
            Ok(()) => panic!("set should not succeed"),
            Err(e) => is_error(e, FxfsError::NoSpace),
        }

        // But inline attributes don't need an attribute number, so it should work fine.
        object
            .set_extended_attribute(
                b"this.is.okay".to_vec(),
                b"small value".to_vec(),
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();

        // And updating existing ones should be okay.
        object
            .set_extended_attribute(b"11".to_vec(), vec![0x4; 300], SetExtendedAttributeMode::Set)
            .await
            .unwrap();
        object
            .set_extended_attribute(
                b"12".to_vec(),
                vec![0x1; 300],
                SetExtendedAttributeMode::Replace,
            )
            .await
            .unwrap();

        // And we should be able to remove an attribute and set another one.
        object.remove_extended_attribute(b"5".to_vec()).await.unwrap();
        object
            .set_extended_attribute(
                b"new attr".to_vec(),
                vec![0x3; 300],
                SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap();

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn write_attr_trims_beyond_new_end() {
        // When writing, multi_write will deallocate old extents that overlap with the new data,
        // but it doesn't trim anything beyond that, since it doesn't know what the total size will
        // be. write_attr does know, because it writes the whole attribute at once, so we need to
        // make sure it cleans up properly.
        let (fs, object) = test_filesystem_and_empty_object().await;

        let block_size = fs.block_size();
        let buf_size = block_size * 2;
        let attribute_id = AttributeId::TEST_ID;

        let mut transaction = (*object).new_transaction(attribute_id).await.unwrap();
        let mut buffer = object.allocate_buffer(buf_size as usize).await;
        buffer.fill(3);
        // Writing two separate ranges, even if they are contiguous, forces them to be separate
        // extent records.
        object
            .multi_write(
                &mut transaction,
                attribute_id,
                &[0..block_size.get(), block_size.get()..block_size * 2],
                buffer.as_mut(),
            )
            .await
            .unwrap();
        transaction.add(
            object.store().store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(object.object_id(), attribute_id, AttributeKey::Attribute),
                ObjectValue::attribute(block_size * 2, false),
            ),
        );
        transaction.commit().await.unwrap();

        crate::fsck::fsck(fs.clone()).await.unwrap();

        let mut transaction = (*object).new_transaction(attribute_id).await.unwrap();
        let needs_trim = (*object)
            .write_attr(&mut transaction, attribute_id, &vec![3u8; block_size.get() as usize])
            .await
            .unwrap();
        assert!(!needs_trim.0);
        transaction.commit().await.unwrap();

        crate::fsck::fsck(fs.clone()).await.unwrap();

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn write_new_attr_in_batches_multiple_txns() {
        let (fs, object) = test_filesystem_and_empty_object().await;
        let merkle_tree = vec![1; 3 * WRITE_ATTR_BATCH_SIZE];
        let mut transaction = (*object).new_transaction(AttributeId::TEST_ID).await.unwrap();
        object
            .write_new_attr_in_batches(
                &mut transaction,
                AttributeId::TEST_ID,
                &merkle_tree,
                WRITE_ATTR_BATCH_SIZE,
            )
            .await
            .expect("failed to write merkle attribute");

        transaction.add(
            object.store().store_object_id,
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_attribute_entry(
                    object.store().graveyard_directory_object_id(),
                    object.object_id(),
                    AttributeId::TEST_ID,
                ),
                ObjectValue::None,
            ),
        );
        transaction.commit().await.unwrap();
        assert_eq!(
            object.read_attr(AttributeId::TEST_ID).await.expect("read_attr failed"),
            Some(merkle_tree.into())
        );

        fs.close().await.expect("close failed");
    }

    // Running on target only, to use fake time features in the executor.
    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test(allow_stalls = false)]
    async fn test_watchdog() {
        use super::Watchdog;
        use fuchsia_async::{MonotonicDuration, MonotonicInstant, TestExecutor};
        use std::sync::mpsc::channel;

        TestExecutor::advance_to(make_time(0)).await;
        let (sender, receiver) = channel();

        fn make_time(time_secs: i64) -> MonotonicInstant {
            MonotonicInstant::from_nanos(0) + MonotonicDuration::from_seconds(time_secs)
        }

        {
            let _watchdog = Watchdog::new(10, move |count| {
                sender.send(count).expect("Sending value");
            });

            // Too early.
            TestExecutor::advance_to(make_time(5)).await;
            receiver.try_recv().expect_err("Should not have message");

            // First message.
            TestExecutor::advance_to(make_time(10)).await;
            assert_eq!(1, receiver.recv().expect("Receiving"));

            // Too early for the next.
            TestExecutor::advance_to(make_time(15)).await;
            receiver.try_recv().expect_err("Should not have message");

            // Missed one. They'll be spooled up.
            TestExecutor::advance_to(make_time(30)).await;
            assert_eq!(2, receiver.recv().expect("Receiving"));
            assert_eq!(3, receiver.recv().expect("Receiving"));
        }

        // Watchdog is dropped, nothing should trigger.
        TestExecutor::advance_to(make_time(100)).await;
        receiver.recv().expect_err("Watchdog should be gone");
    }

    #[fuchsia::test]
    fn test_checksum_range_chunk() {
        let block_size = BlockSize::SIZE_4KIB;

        // No bitmap means one chunk that covers the whole range
        assert_eq!(
            ChecksumRangeChunk::group_first_write_ranges(
                &mut OverwriteBitmaps::None,
                block_size,
                block_size * 2..block_size * 5,
            ),
            vec![ChecksumRangeChunk {
                checksum_range: 0..3,
                device_range: block_size * 2..block_size * 5,
                is_first_write: false,
            }],
        );

        let mut bitmaps = OverwriteBitmaps::new(BitVec::from_bytes(&[0b11110000]));
        assert_eq!(
            ChecksumRangeChunk::group_first_write_ranges(
                &mut bitmaps,
                block_size,
                block_size * 2..block_size * 5,
            ),
            vec![ChecksumRangeChunk {
                checksum_range: 0..3,
                device_range: block_size * 2..block_size * 5,
                is_first_write: false,
            }],
        );
        assert_eq!(
            bitmaps.take_bitmaps(),
            Some((BitVec::from_bytes(&[0b11110000]), BitVec::from_bytes(&[0b11100000])))
        );

        let mut bitmaps = OverwriteBitmaps::new(BitVec::from_bytes(&[0b11110000]));
        bitmaps.set_offset(2);
        assert_eq!(
            ChecksumRangeChunk::group_first_write_ranges(
                &mut bitmaps,
                block_size,
                block_size * 2..block_size * 5,
            ),
            vec![
                ChecksumRangeChunk {
                    checksum_range: 0..2,
                    device_range: block_size * 2..block_size * 4,
                    is_first_write: false,
                },
                ChecksumRangeChunk {
                    checksum_range: 2..3,
                    device_range: block_size * 4..block_size * 5,
                    is_first_write: true,
                },
            ],
        );
        assert_eq!(
            bitmaps.take_bitmaps(),
            Some((BitVec::from_bytes(&[0b11110000]), BitVec::from_bytes(&[0b00111000])))
        );

        let mut bitmaps = OverwriteBitmaps::new(BitVec::from_bytes(&[0b11110000]));
        bitmaps.set_offset(4);
        assert_eq!(
            ChecksumRangeChunk::group_first_write_ranges(
                &mut bitmaps,
                block_size,
                block_size * 2..block_size * 5,
            ),
            vec![ChecksumRangeChunk {
                checksum_range: 0..3,
                device_range: block_size * 2..block_size * 5,
                is_first_write: true,
            }],
        );
        assert_eq!(
            bitmaps.take_bitmaps(),
            Some((BitVec::from_bytes(&[0b11110000]), BitVec::from_bytes(&[0b00001110])))
        );

        let mut bitmaps = OverwriteBitmaps::new(BitVec::from_bytes(&[0b01010101]));
        assert_eq!(
            ChecksumRangeChunk::group_first_write_ranges(
                &mut bitmaps,
                block_size,
                block_size * 2..block_size * 10,
            ),
            vec![
                ChecksumRangeChunk {
                    checksum_range: 0..1,
                    device_range: block_size * 2..block_size * 3,
                    is_first_write: true,
                },
                ChecksumRangeChunk {
                    checksum_range: 1..2,
                    device_range: block_size * 3..block_size * 4,
                    is_first_write: false,
                },
                ChecksumRangeChunk {
                    checksum_range: 2..3,
                    device_range: block_size * 4..block_size * 5,
                    is_first_write: true,
                },
                ChecksumRangeChunk {
                    checksum_range: 3..4,
                    device_range: block_size * 5..block_size * 6,
                    is_first_write: false,
                },
                ChecksumRangeChunk {
                    checksum_range: 4..5,
                    device_range: block_size * 6..block_size * 7,
                    is_first_write: true,
                },
                ChecksumRangeChunk {
                    checksum_range: 5..6,
                    device_range: block_size * 7..block_size * 8,
                    is_first_write: false,
                },
                ChecksumRangeChunk {
                    checksum_range: 6..7,
                    device_range: block_size * 8..block_size * 9,
                    is_first_write: true,
                },
                ChecksumRangeChunk {
                    checksum_range: 7..8,
                    device_range: block_size * 9..block_size * 10,
                    is_first_write: false,
                },
            ],
        );
        assert_eq!(
            bitmaps.take_bitmaps(),
            Some((BitVec::from_bytes(&[0b01010101]), BitVec::from_bytes(&[0b11111111])))
        );
    }

    #[fuchsia::test]
    async fn test_read_large_buffer_excessive_partitions() {
        let (_fs, object) = test_filesystem_and_empty_object().await;

        // Write 5 MiB of data so that reading it spans > MAX_HASH_PARTITIONS (5 > 4 partitions).
        let size = 5 * 1024 * 1024;
        let mut buf = object.allocate_buffer(size).await;
        buf.as_mut_ptr_slice().fill(0xab);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        // Flush to create persistent layers with bloom filters.
        object.owner().flush().await.expect("flush failed");

        // Read the entire 5 MiB buffer back.
        let mut read_buf = object.allocate_buffer(size).await;
        assert_eq!(object.read(0, read_buf.as_mut()).await.expect("read failed"), size);
        assert_eq!(&read_buf.as_ptr_slice().to_vec()[..], &buf.as_ptr_slice().to_vec()[..]);
    }
}
