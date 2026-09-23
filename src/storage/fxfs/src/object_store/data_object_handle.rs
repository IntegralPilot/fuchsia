// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use crate::log::*;
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::{
    ObjectHandle, ObjectProperties, ReadObjectHandle, WriteBytes, WriteObjectHandle,
};
use crate::object_store::extent_record::{ExtentMode, ExtentValue};
use crate::object_store::object_manager::ObjectManager;
use crate::object_store::object_record::{
    AttributeKey, DirType, FsverityMetadata, ObjectAttributes, ObjectItem, ObjectKey,
    ObjectKeyData, ObjectKind, ObjectValue, Timestamp,
};
use crate::object_store::store_object_handle::{MaybeChecksums, NeedsTrim};
use crate::object_store::transaction::{
    self, AssocObj, AssociatedObject, LockKey, Mutation, ObjectStoreMutation, Operation, Options,
    Transaction, lock_keys,
};
use crate::object_store::{
    AttributeId, Extent, HandleOptions, HandleOwner, RootDigest, StoreObjectHandle,
    TRANSACTION_MUTATION_THRESHOLD, TrimMode, TrimResult,
};
use crate::range::RangeExt;
use anyhow::{Context, Error, anyhow, bail, ensure};
use fidl_fuchsia_io as fio;
use fsverity_merkle::{
    FsVerityDescriptor, FsVerityDescriptorRaw, FsVerityHash, FsVerityHasher, FsVerityHasherOptions,
    MerkleTree, MerkleTreeBuilder, Sha256Hash, Sha512Hash,
};
use fuchsia_sync::Mutex;
use futures::TryStreamExt;
use futures::stream::FuturesOrdered;
use fxfs_trace::trace;
use std::cmp::min;
use std::future::Future;
use std::ops::{Deref, Range};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicU64, Ordering};
use storage_device::WriteFlags;
use storage_device::buffer::{Buffer, BufferFuture, BufferRef, MutableBufferRef};
use storage_ptr_slice::PtrByteSlice;
use storage_units::BlockSize;
use zerocopy::FromBytes;

mod allocated_ranges;
pub use allocated_ranges::{AllocatedRanges, RangeType};

/// How much data each transaction will cover when writing an attribute across batches. Pulled from
/// `FLUSH_BATCH_SIZE` in paged_object_handle.rs.
pub const WRITE_ATTR_BATCH_SIZE: usize = 524_288;

/// DataObjectHandle is a typed handle for file-like objects that store data in the default data
/// attribute. In addition to traditional files, this means things like the journal, superblocks,
/// and layer files.
///
/// It caches the content size of the data attribute it was configured for, and has helpers for
/// complex extent manipulation, as well as implementations of ReadObjectHandle and
/// WriteObjectHandle.
pub struct DataObjectHandle<S: HandleOwner> {
    handle: StoreObjectHandle<S>,
    attribute_id: AttributeId,
    content_size: AtomicU64,
    state: Mutex<DataObjectState>,
}

/// Represents the mapping of a file's contents to the physical storage backing it.
#[derive(Debug, Clone)]
pub struct FileExtent {
    logical_offset: u64,
    device_range: Range<u64>,
}

impl FileExtent {
    pub fn new(logical_offset: u64, device_range: Range<u64>) -> Result<Self, Error> {
        // Ensure `device_range` is valid.
        let length = device_range.length()?;
        // Ensure no overflow when we calculate the end of the logical range.
        let _ = logical_offset.checked_add(length).ok_or(FxfsError::OutOfRange)?;
        Ok(Self { logical_offset, device_range })
    }
}

impl FileExtent {
    pub fn length(&self) -> u64 {
        // SAFETY: We verified that the device_range's length is valid in Self::new.
        unsafe { self.device_range.unchecked_length() }
    }

    pub fn logical_offset(&self) -> u64 {
        self.logical_offset
    }

    pub fn logical_range(&self) -> Range<u64> {
        // SAFETY: We verified logical_offset plus device_range length won't overflow in Self::new.
        unsafe { self.logical_offset..self.logical_offset.unchecked_add(self.length()) }
    }

    pub fn device_range(&self) -> &Range<u64> {
        &self.device_range
    }
}

#[derive(Debug)]
pub enum DataObjectState {
    Standard(AllocatedRanges),
    VerityStarted,
    VerityPending(FsverityStateInner),
    Verity(FsverityStateInner),
}

#[derive(Debug)]
pub struct FsverityStateInner {
    root_digest: RootDigest,
    salt: Vec<u8>,
    // TODO(b/309656632): This should store the entire merkle tree and not just the leaf nodes.
    // Potentially store a pager-backed vmo instead of passing around a boxed array.
    merkle_tree: Box<[u8]>,
}

#[derive(Debug, Default)]
pub struct OverwriteOptions {
    // If false, then all the extents for the overwrite range must have been preallocated using
    // preallocate_range or from existing writes.
    pub allow_allocations: bool,
    pub barrier_on_first_write: bool,
}

impl FsverityStateInner {
    pub fn new(root_digest: RootDigest, salt: Vec<u8>, merkle_tree: Box<[u8]>) -> Self {
        FsverityStateInner { root_digest, salt, merkle_tree }
    }

    fn get_hasher_for_block_size(&self, block_size: BlockSize) -> FsVerityHasher {
        match self.root_digest {
            RootDigest::Sha256(_) => FsVerityHasher::Sha256(FsVerityHasherOptions::new(
                self.salt.clone(),
                block_size.get() as usize,
            )),
            RootDigest::Sha512(_) => FsVerityHasher::Sha512(FsVerityHasherOptions::new(
                self.salt.clone(),
                block_size.get() as usize,
            )),
        }
    }

    fn from_ptr_slice(
        data: PtrByteSlice<'_>,
        block_size: BlockSize,
    ) -> Result<(Self, FsVerityHasher), Error> {
        let descriptor = FsVerityDescriptor::new(data, block_size.get() as usize)
            .map_err(|e| anyhow!(FxfsError::IntegrityError).context(e))?;

        let root_digest = match descriptor.digest_algorithm() {
            fio::HashAlgorithm::Sha256 => {
                RootDigest::Sha256(descriptor.root_digest().try_into().unwrap())
            }
            fio::HashAlgorithm::Sha512 => RootDigest::Sha512(descriptor.root_digest().to_vec()),
            _ => return Err(anyhow!(FxfsError::NotSupported).context("Unsupported hash algorithm")),
        };
        let hasher = descriptor.hasher();
        let leaves =
            descriptor.leaf_digests().map_err(|e| anyhow!(FxfsError::IntegrityError).context(e))?;

        Ok((Self::new(root_digest, descriptor.salt().to_vec(), leaves.into_boxed_slice()), hasher))
    }
}

impl<S: HandleOwner> Deref for DataObjectHandle<S> {
    type Target = StoreObjectHandle<S>;
    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

impl<S: HandleOwner> DataObjectHandle<S> {
    pub fn new(
        owner: Arc<S>,
        object_id: u64,
        permanent_keys: bool,
        attribute_id: AttributeId,
        size: u64,
        options: HandleOptions,
        trace: bool,
        overwrite_ranges: &[Range<u64>],
    ) -> Self {
        Self {
            handle: StoreObjectHandle::new(owner, object_id, permanent_keys, options, trace),
            attribute_id,
            content_size: AtomicU64::new(size),
            state: Mutex::new(DataObjectState::Standard(AllocatedRanges::new(overwrite_ranges))),
        }
    }

    pub fn attribute_id(&self) -> AttributeId {
        self.attribute_id
    }

    /// Consumes the `DataObjectHandle` and returns the `StoreObjectHandle` that it contained.
    pub fn into_store_object_handle(self) -> StoreObjectHandle<S> {
        self.handle
    }

    pub fn overwrite_ranges_is_empty(&self) -> bool {
        match &*self.state.lock() {
            DataObjectState::Standard(ranges) => ranges.is_empty(),
            _ => true,
        }
    }

    pub fn with_overwrite_ranges<R>(&self, f: impl FnOnce(Option<&AllocatedRanges>) -> R) -> R {
        let state = self.state.lock();
        match &*state {
            DataObjectState::Standard(ranges) => f(Some(ranges)),
            _ => f(None),
        }
    }

    pub fn with_overwrite_ranges_mut<R>(
        &self,
        f: impl FnOnce(Option<&mut AllocatedRanges>) -> R,
    ) -> R {
        let mut state = self.state.lock();
        match &mut *state {
            DataObjectState::Standard(ranges) => f(Some(ranges)),
            _ => f(None),
        }
    }

    pub fn is_verified_file(&self) -> bool {
        matches!(*self.state.lock(), DataObjectState::Verity(_))
    }

    /// Sets `self.state` to DataObjectState::VerityStarted. Called at the top of `enable_verity`.
    /// If another caller has already started but not completed `enable_verity`, returns
    /// FxfsError::AlreadyBound. If another caller has already completed `enable_verity`, returns
    /// FxfsError::AlreadyExists.
    ///
    /// Note: This is called before `enable_verity` acquires its object transaction lock. Any
    /// ongoing transaction holding the object lock (such as `allocate`) will proceed to commit
    /// its mutations to disk while `enable_verity` waits for the lock, and `will_apply_mutation`
    /// will gracefully discard in-memory range updates for the discarded `AllocatedRanges`.
    pub fn set_fsverity_state_started(&self) -> Result<(), Error> {
        let mut state = self.state.lock();
        match *state {
            DataObjectState::Standard(_) => {
                *state = DataObjectState::VerityStarted;
                Ok(())
            }
            DataObjectState::VerityStarted | DataObjectState::VerityPending(_) => {
                Err(anyhow!(FxfsError::Unavailable))
            }
            DataObjectState::Verity(_) => Err(anyhow!(FxfsError::AlreadyExists)),
        }
    }

    /// Sets `self.state` to VerityPending. Must be called before `finalize_fsverity_state()`.
    ///
    /// # Panics
    ///
    /// Panics if the prior state was not `DataObjectState::VerityStarted`.
    pub fn set_fsverity_state_pending(&self, descriptor: FsverityStateInner) {
        let mut state = self.state.lock();
        assert!(matches!(*state, DataObjectState::VerityStarted));
        *state = DataObjectState::VerityPending(descriptor);
    }

    /// Sets `self.state` to Verity.
    ///
    /// # Panics
    ///
    /// Panics if the prior state was not `DataObjectState::VerityPending(_)`.
    pub fn finalize_fsverity_state(&self) {
        let mut state = self.state.lock();
        let old_state =
            std::mem::replace(&mut *state, DataObjectState::Standard(AllocatedRanges::empty()));
        match old_state {
            DataObjectState::VerityPending(inner) => *state = DataObjectState::Verity(inner),
            _ => panic!("Cannot finalize verity state from {old_state:?}"),
        }
    }

    /// Sets `self.state` directly to Verity without going through the entire state machine.
    /// Used to set `self.state` on open of a verified file. The merkle tree data is
    /// verified against the root digest here, and will return an error if the tree is not correct.
    ///
    /// # Panics
    ///
    /// Panics if the prior state was not `DataObjectState::Standard(_)`.
    pub async fn set_fsverity_state_some(&self, descriptor: FsverityMetadata) -> Result<(), Error> {
        let (metadata, hasher) = match descriptor {
            FsverityMetadata::Internal(root_digest, salt) => {
                let merkle_tree = self
                    .read_attr(AttributeId::FSVERITY_MERKLE)
                    .await?
                    .ok_or_else(|| anyhow!(FxfsError::Inconsistent))?;
                let metadata = FsverityStateInner { root_digest, salt, merkle_tree };
                let hasher = metadata.get_hasher_for_block_size(self.block_size());
                (metadata, hasher)
            }
            FsverityMetadata::F2fs(verity_range) => {
                let expected_length = verity_range.length()? as usize;
                let mut buffer = self
                    .allocate_buffer(
                        self.block_size().align_up(expected_length as u64).unwrap() as usize
                    )
                    .await;
                ensure!(
                    expected_length
                        == self
                            .handle
                            .read(AttributeId::FSVERITY_MERKLE, verity_range.start, buffer.as_mut())
                            .await?,
                    FxfsError::Inconsistent
                );
                let data = buffer.as_ptr_slice().subslice(0..expected_length);
                FsverityStateInner::from_ptr_slice(data, self.block_size())?
            }
        };
        // Validate the merkle tree data against the root before applying it.
        ensure!(metadata.merkle_tree.len() % hasher.hash_size() == 0, FxfsError::Inconsistent);
        let leaf_chunks = metadata.merkle_tree.chunks_exact(hasher.hash_size());

        let root_hash = match &metadata.root_digest {
            RootDigest::Sha256(root_hash) => root_hash.as_slice(),
            RootDigest::Sha512(root_hash) => root_hash.as_slice(),
        };

        let tree = match hasher {
            FsVerityHasher::Sha256(_) => {
                let mut builder = MerkleTreeBuilder::<Sha256Hash>::new(hasher);
                for leaf in leaf_chunks {
                    let hash = Sha256Hash::read_from_bytes(leaf).unwrap();
                    builder.push_data_hash(hash);
                }
                builder.finish()
            }
            FsVerityHasher::Sha512(_) => {
                let mut builder = MerkleTreeBuilder::<Sha512Hash>::new(hasher);
                for leaf in leaf_chunks {
                    let hash = Sha512Hash::read_from_bytes(leaf).unwrap();
                    builder.push_data_hash(hash);
                }
                builder.finish()
            }
        };

        ensure!(root_hash == tree.root(), FxfsError::IntegrityError);

        let mut state = self.state.lock();
        assert!(matches!(*state, DataObjectState::Standard(_)));
        *state = DataObjectState::Verity(metadata);

        Ok(())
    }

    /// Verifies contents of `buffer` against the corresponding hashes in the stored merkle tree.
    /// `offset` is the logical offset in the file that `buffer` starts at. Fails on non
    /// fsverity-enabled files.
    ///
    /// # Panics
    ///
    /// Panics if `offset` is not block-aligned.
    fn verify_data(&self, mut offset: usize, buffer: PtrByteSlice<'_>) -> Result<(), Error> {
        let block_size = self.block_size();
        assert!(block_size.is_aligned(offset as u64));
        let state = self.state.lock();
        match &*state {
            DataObjectState::Standard(_) => {
                Err(anyhow!("Tried to verify read on a non verity-enabled file"))
            }
            DataObjectState::VerityStarted | DataObjectState::VerityPending(_) => {
                Err(anyhow!("Enable verity has not yet completed, state: {state:?}"))
            }
            DataObjectState::Verity(metadata) => {
                let hasher = metadata.get_hasher_for_block_size(block_size);
                let leaf_nodes: Vec<&[u8]> =
                    metadata.merkle_tree.chunks(hasher.hash_size()).collect();
                fxfs_trace::duration!("fsverity-verify", "len" => buffer.len());
                // TODO(b/318880297): Consider parallelizing computation.
                for chunk in buffer.chunks(block_size.get() as usize) {
                    // SAFETY: Ideally we wouldn't be creating references here as technically this is
                    // Rust undefined behaviour, but it's difficult for us to fix and mitigated
                    // because these pointers end up being passed directly to non-Rust code (Mundane).
                    let b = unsafe { &*chunk.as_raw_slice_ptr() };

                    ensure!(
                        hasher.hash_block(b) == leaf_nodes[((offset as u64) / block_size) as usize],
                        anyhow!(FxfsError::Inconsistent).context("Hash mismatch")
                    );
                    offset += block_size.get() as usize;
                }
                Ok(())
            }
        }
    }

    /// Extend the file with the given extent.  The only use case for this right now is for files
    /// that must exist at certain offsets on the device, such as super-blocks.
    pub async fn extend<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        device_range: Range<u64>,
    ) -> Result<(), Error> {
        let old_end =
            self.block_size().align_up(self.txn_get_size(transaction)).ok_or(FxfsError::TooBig)?;
        let new_size = old_end + device_range.end - device_range.start;
        self.store().allocator().mark_allocated(
            transaction,
            self.store().store_object_id(),
            device_range.clone(),
        )?;
        self.txn_update_size(transaction, new_size, None).await?;
        let key_id = self.get_key(None).await?.0;
        transaction.add(
            self.store().store_object_id,
            Mutation::merge_object(
                ObjectKey::extent(self.object_id(), self.attribute_id(), old_end..new_size),
                ObjectValue::Extent(ExtentValue::new_raw(device_range.start, key_id)),
            ),
        );
        self.update_allocated_size(transaction, device_range.end - device_range.start, 0).await
    }

    // Returns a new aligned buffer (reading the head and tail blocks if necessary) with a copy of
    // the data from `buf`.
    async fn align_buffer(
        &self,
        offset: u64,
        buf: BufferRef<'_>,
    ) -> Result<(std::ops::Range<u64>, Buffer<'_>), Error> {
        self.handle.align_buffer(self.attribute_id(), offset, buf).await
    }

    // Writes potentially unaligned data at `device_offset` and returns checksums if requested. The
    // data will be encrypted if necessary.
    // `buf` is mutable as an optimization, since the write may require encryption, we can encrypt
    // the buffer in-place rather than copying to another buffer if the write is already aligned.
    // `flags` are forwarded to the underlying device write.
    async fn write_at(
        &self,
        offset: u64,
        buf: MutableBufferRef<'_>,
        device_offset: u64,
        flags: WriteFlags,
    ) -> Result<MaybeChecksums, Error> {
        self.handle
            .write_at_with_flags(self.attribute_id(), offset, buf, None, device_offset, flags)
            .await
    }

    /// Verifies that the entire range in the file is zeroes, as either uninitialized overwrite
    /// range, or no extent at all. If a single allocated and written extent is found, this returns
    /// false.
    pub async fn check_unwritten_zero(&self, range: Range<u64>) -> Result<bool, Error> {
        let tree = &self.store().tree();
        let layer_set = tree.layer_set();
        let key = Extent(range);
        let lower_bound = ObjectKey::attribute(
            self.object_id(),
            self.attribute_id,
            AttributeKey::Extent(key.search_key()),
        );
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullRange(&lower_bound)).await?;
        while let Some(ItemRef {
            key:
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(attr_id, AttributeKey::Extent(extent_key)),
                },
            value: ObjectValue::Extent(value),
            ..
        }) = iter.get()
            && *object_id == self.object_id()
            && *attr_id == self.attribute_id
        {
            if let ExtentValue::Some { mode, .. } = value {
                if let Some(overlap) = key.overlap(extent_key) {
                    if let ExtentMode::OverwritePartial(bits) = mode {
                        let starting_index = (overlap.start - extent_key.start) / self.block_size();
                        for initialized in bits
                            .iter()
                            .skip(starting_index as usize)
                            .take((overlap.length().unwrap() / self.block_size()) as usize)
                        {
                            if initialized {
                                return Ok(false);
                            }
                        }
                    } else {
                        return Ok(false);
                    }
                } else {
                    break;
                }
            }
            iter.advance().await?;
        }
        Ok(true)
    }

    /// Zeroes the given range.  The range must be aligned.  Returns the amount of data deallocated.
    pub async fn zero(
        &self,
        transaction: &mut Transaction<'_>,
        range: Range<u64>,
    ) -> Result<(), Error> {
        self.handle.zero(transaction, self.attribute_id(), range).await
    }

    /// The cached value for `self.fsverity_state` is set either in `open_object` or on
    /// `enable_verity`. If set, translates `self.fsverity_state.descriptor` into an
    /// fio::VerificationOptions instance and a root hash. Otherwise, returns None.
    pub fn get_descriptor(&self) -> Option<(fio::VerificationOptions, Vec<u8>)> {
        let state = self.state.lock();
        match &*state {
            DataObjectState::Verity(metadata) => {
                let (options, root_hash) = match &metadata.root_digest {
                    RootDigest::Sha256(root_hash) => (
                        fio::VerificationOptions {
                            hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                            salt: Some(metadata.salt.clone()),
                            ..Default::default()
                        },
                        root_hash.to_vec(),
                    ),
                    RootDigest::Sha512(root_hash) => (
                        fio::VerificationOptions {
                            hash_algorithm: Some(fio::HashAlgorithm::Sha512),
                            salt: Some(metadata.salt.clone()),
                            ..Default::default()
                        },
                        root_hash.clone(),
                    ),
                };
                Some((options, root_hash))
            }
            _ => None,
        }
    }

    async fn build_verity_tree(
        &self,
        hasher: FsVerityHasher,
        hash_alg: fio::HashAlgorithm,
        salt: &[u8],
    ) -> Result<(MerkleTree, Vec<u8>), Error> {
        match hasher {
            FsVerityHasher::Sha256(_) => {
                self.build_verity_tree_impl::<Sha256Hash>(hasher, hash_alg, salt).await
            }
            FsVerityHasher::Sha512(_) => {
                self.build_verity_tree_impl::<Sha512Hash>(hasher, hash_alg, salt).await
            }
        }
    }

    async fn build_verity_tree_impl<D: FsVerityHash>(
        &self,
        hasher: FsVerityHasher,
        hash_alg: fio::HashAlgorithm,
        salt: &[u8],
    ) -> Result<(MerkleTree, Vec<u8>), Error> {
        let hash_len = hasher.hash_size();
        let mut builder = MerkleTreeBuilder::<D>::new(hasher);
        let mut offset = 0;
        let size = self.get_size();
        // TODO(b/314836822): Consider further tuning the buffer size to optimize
        // performance. Experimentally, most verity-enabled files are <256K.
        let mut buf = self.allocate_buffer(64 * self.block_size().get() as usize).await;
        while offset < size {
            // TODO(b/314842875): Consider optimizations for sparse files.
            let read = self.read(offset, buf.as_mut()).await? as u64;
            assert!(offset + read <= size);
            let slice = buf.as_ptr_slice().subslice(0..read as usize);

            // SAFETY: Ideally we wouldn't be creating references here as technically this is
            // Rust undefined behaviour, but it's difficult for us to fix and mitigated
            // because these pointers end up being passed directly to non-Rust code (Mundane).
            let chunk = unsafe { &*slice.as_raw_slice_ptr() };

            builder.write(chunk);
            offset += read;
        }
        let tree = builder.finish();
        // This will include a block for the root layer, which will be used to house the descriptor.
        let tree_data_len = tree
            .levels()
            .iter()
            .map(|layer| self.block_size().align_up(layer.len() as u64).unwrap() as usize)
            .sum();
        let mut merkle_tree_data = Vec::<u8>::with_capacity(tree_data_len);
        // Iterating from the top layers down to the leaves.
        for layer in tree.levels().iter().rev() {
            // Skip the root layer.
            if layer.len() <= hash_len {
                continue;
            }
            merkle_tree_data.extend_from_slice(layer);
            // Pad to the end of the block.
            let padded_size =
                self.block_size().align_up(merkle_tree_data.len() as u64).unwrap() as usize;
            merkle_tree_data.resize(padded_size, 0);
        }

        // Zero the last block, then write the descriptor to the start of it.
        let descriptor_offset = merkle_tree_data.len();
        merkle_tree_data.resize(descriptor_offset + self.block_size().get() as usize, 0);
        let descriptor = FsVerityDescriptorRaw::new(
            hash_alg,
            self.block_size().get(),
            self.get_size(),
            tree.root(),
            salt,
        )?;
        descriptor.write_to_slice(&mut merkle_tree_data[descriptor_offset..])?;

        Ok((tree, merkle_tree_data))
    }

    /// Reads the data attribute and computes a merkle tree from the data. The values of the
    /// parameters required to build the merkle tree are supplied by `descriptor` (i.e. salt,
    /// hash_algorithm, etc.) Writes the leaf nodes of the merkle tree to an attribute with id
    /// `AttributeId::FSVERITY_MERKLE`. Updates the root_hash of the `descriptor` according to the
    /// computed merkle tree and then replaces the ObjectValue of the data attribute with
    /// ObjectValue::VerifiedAttribute, which stores the `descriptor` inline.
    #[trace]
    pub async fn enable_verity(&self, options: fio::VerificationOptions) -> Result<(), Error> {
        self.set_fsverity_state_started()?;
        // If the merkle attribute was tombstoned in the last attempt of `enable_verity`, flushing
        // the graveyard should process the tombstone before we start rewriting the attribute.
        if self
            .store()
            .tree()
            .exists(&ObjectKey::graveyard_attribute_entry(
                self.store().graveyard_directory_object_id(),
                self.object_id(),
                AttributeId::FSVERITY_MERKLE,
            ))
            .await?
        {
            self.store().filesystem().graveyard().flush().await;
        }
        let mut transaction = self.new_transaction().await?;
        let hash_alg =
            options.hash_algorithm.ok_or_else(|| anyhow!("No hash algorithm provided"))?;
        let salt = options.salt.ok_or_else(|| anyhow!("No salt provided"))?;
        let (root_digest, merkle_tree) = match hash_alg {
            fio::HashAlgorithm::Sha256 => {
                let hasher = FsVerityHasher::Sha256(FsVerityHasherOptions::new(
                    salt.clone(),
                    self.block_size().get() as usize,
                ));
                let (tree, merkle_tree_data) =
                    self.build_verity_tree(hasher, hash_alg, &salt).await?;
                let root: [u8; 32] = tree.root().try_into().unwrap();
                (RootDigest::Sha256(root), merkle_tree_data)
            }
            fio::HashAlgorithm::Sha512 => {
                let hasher = FsVerityHasher::Sha512(FsVerityHasherOptions::new(
                    salt.clone(),
                    self.block_size().get() as usize,
                ));
                let (tree, merkle_tree_data) =
                    self.build_verity_tree(hasher, hash_alg, &salt).await?;
                (RootDigest::Sha512(tree.root().to_vec()), merkle_tree_data)
            }
            _ => {
                bail!(
                    anyhow!(FxfsError::NotSupported)
                        .context(format!("hash algorithm not supported"))
                );
            }
        };
        // TODO(b/314194485): Eventually want streaming writes.
        // The merkle tree attribute should not require trimming because it should not
        // exist.
        self.handle
            .write_new_attr_in_batches(
                &mut transaction,
                AttributeId::FSVERITY_MERKLE,
                &merkle_tree,
                WRITE_ATTR_BATCH_SIZE,
            )
            .await?;
        if merkle_tree.len() > WRITE_ATTR_BATCH_SIZE {
            self.store().remove_attribute_from_graveyard(
                &mut transaction,
                self.object_id(),
                AttributeId::FSVERITY_MERKLE,
            );
        };
        let descriptor_decoded =
            FsVerityDescriptor::new(&merkle_tree[..], self.block_size().get() as usize)?;
        let descriptor = FsverityStateInner {
            root_digest,
            salt,
            merkle_tree: descriptor_decoded.leaf_digests()?.into(),
        };
        self.set_fsverity_state_pending(descriptor);
        transaction.add_with_object(
            self.store().store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(self.object_id(), AttributeId::DATA, AttributeKey::Attribute),
                ObjectValue::verified_attribute(
                    self.get_size(),
                    FsverityMetadata::F2fs(0..merkle_tree.len() as u64),
                ),
            ),
            AssocObj::Borrowed(self),
        );
        transaction.commit().await?;
        Ok(())
    }

    /// Pre-allocate disk space for the given logical file range. If any part of the allocation
    /// range is beyond the end of the file, the file size is updated.
    pub async fn allocate(&self, range: Range<u64>) -> Result<(), Error> {
        debug_assert!(range.start < range.end);

        // It's not required that callers of allocate use block aligned ranges, but we need to make
        // the extents block aligned. Luckily, fallocate in posix is allowed to allocate more than
        // what was asked for for block alignment purposes. We just need to make sure that the size
        // NB: FxfsError::TooBig turns into EFBIG when passed through starnix, which is the
        // required error code when the requested range is larger than the file size.
        let mut new_range =
            self.block_size().align_range_outwards(&range).ok_or(FxfsError::TooBig)?;

        let mut transaction = self.new_transaction().await?;
        // It's safe to check state after acquiring the transaction lock. Note that `enable_verity`
        // calls `set_fsverity_state_started` before creating its own transaction, which could
        // transition `self.state` to `VerityStarted` concurrently while this transaction is held.
        // If that happens, `enable_verity`'s subsequent `new_transaction` call will block until
        // this allocation transaction finishes, and `will_apply_mutation` will gracefully discard
        // updates to the in-memory allocated ranges.
        {
            let state = self.state.lock();
            match &*state {
                DataObjectState::Standard(_) => {}
                _ => bail!(
                    anyhow!(FxfsError::AccessDenied).context("Cannot allocate on verity file")
                ),
            }
        }
        let mut to_allocate = Vec::new();
        let mut to_switch = Vec::new();
        let key_id = self.get_key(None).await?.0;

        {
            let tree = &self.store().tree;
            let layer_set = tree.layer_set();
            let offset_key = ObjectKey::attribute(
                self.object_id(),
                self.attribute_id(),
                AttributeKey::Extent(Extent::search_key_from_offset(new_range.start)),
            );
            let mut merger = layer_set.merger();
            let mut iter = merger.query(Query::FullRange(&offset_key)).await?;

            loop {
                match iter.get() {
                    Some(ItemRef {
                        key:
                            ObjectKey {
                                object_id,
                                data:
                                    ObjectKeyData::Attribute(
                                        attribute_id,
                                        AttributeKey::Extent(extent_key),
                                    ),
                            },
                        value: ObjectValue::Extent(extent_value),
                        ..
                    }) if *object_id == self.object_id()
                        && *attribute_id == self.attribute_id() =>
                    {
                        // If the start of this extent is beyond the end of the range we are
                        // allocating, we don't have any more work to do.
                        if new_range.end <= extent_key.start {
                            break;
                        }
                        // Add any prefix we might need to allocate.
                        if new_range.start < extent_key.start {
                            to_allocate.push(new_range.start..extent_key.start);
                            new_range.start = extent_key.start;
                        }
                        let device_offset = match extent_value {
                            ExtentValue::None => {
                                // If the extent value is None, it indicates a deleted extent. In
                                // that case, we just skip it entirely. By keeping the new_range
                                // where it is, this section will get included in the new
                                // allocations.
                                iter.advance().await?;
                                continue;
                            }
                            ExtentValue::Some { mode: ExtentMode::OverwritePartial(_), .. }
                            | ExtentValue::Some { mode: ExtentMode::Overwrite, .. } => {
                                // If this extent is already in overwrite mode, we can skip it.
                                if extent_key.end < new_range.end {
                                    new_range.start = extent_key.end;
                                    iter.advance().await?;
                                    continue;
                                } else {
                                    new_range.start = new_range.end;
                                    break;
                                }
                            }
                            ExtentValue::Some { device_offset, .. } => *device_offset,
                        };

                        // Figure out how we have to break up the ranges.
                        let device_offset = device_offset + (new_range.start - extent_key.start);
                        if extent_key.end < new_range.end {
                            to_switch.push((new_range.start..extent_key.end, device_offset));
                            new_range.start = extent_key.end;
                        } else {
                            to_switch.push((new_range.start..new_range.end, device_offset));
                            new_range.start = new_range.end;
                            break;
                        }
                    }
                    // The records are sorted so if we find something that isn't an extent or
                    // doesn't match the object id then there are no more extent records for this
                    // object.
                    _ => break,
                }
                iter.advance().await?;
            }
        }

        if new_range.start < new_range.end {
            to_allocate.push(new_range.clone());
        }

        // We can update the size in the first transaction because even if subsequent transactions
        // don't get replayed, the data between the current and new end of the file will be zero
        // (either sparse zero or allocated zero). On the other hand, if we don't update the size
        // in the first transaction, overwrite extents may be written past the end of the file
        // which is an fsck error.
        //
        // The potential new size needs to be the non-block-aligned range end - we round up to the
        // nearest block size for the actual allocation, but shouldn't do that for the file size.
        let new_size = std::cmp::max(range.end, self.get_size());
        // Make sure the mutation that flips the has_overwrite_extents advisory flag is in the
        // first transaction, in case we split transactions. This makes it okay to only replay the
        // first transaction if power loss occurs - the file will be in an unusual state, but not
        // an invalid one, if only part of the allocate goes through.
        transaction.add_with_object(
            self.store().store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    self.object_id(),
                    self.attribute_id(),
                    AttributeKey::Attribute,
                ),
                ObjectValue::Attribute { size: new_size, has_overwrite_extents: true },
            ),
            AssocObj::Borrowed(self),
        );

        // The maximum number of mutations we are going to allow per transaction in allocate. This
        // is probably quite a bit lower than the actual limit, but it should be large enough to
        // handle most non-edge-case versions of allocate without splitting the transaction.
        const MAX_TRANSACTION_SIZE: usize = 256;
        for (switch_range, device_offset) in to_switch {
            transaction.add_with_object(
                self.store().store_object_id(),
                Mutation::merge_object(
                    ObjectKey::extent(self.object_id(), self.attribute_id(), switch_range),
                    ObjectValue::Extent(ExtentValue::initialized_overwrite_extent(
                        device_offset,
                        key_id,
                    )),
                ),
                AssocObj::Borrowed(self),
            );
            if transaction.mutations().len() >= MAX_TRANSACTION_SIZE {
                transaction.commit_and_continue().await?;
            }
        }

        let mut allocated = 0;
        let allocator = self.store().allocator();
        for mut allocate_range in to_allocate {
            while allocate_range.start < allocate_range.end {
                let device_range = allocator
                    .allocate(
                        &mut transaction,
                        self.store().store_object_id(),
                        allocate_range.end - allocate_range.start,
                    )
                    .await
                    .context("allocation failed")?;
                let device_range_len = device_range.end - device_range.start;

                transaction.add_with_object(
                    self.store().store_object_id(),
                    Mutation::merge_object(
                        ObjectKey::extent(
                            self.object_id(),
                            self.attribute_id(),
                            allocate_range.start..allocate_range.start + device_range_len,
                        ),
                        ObjectValue::Extent(ExtentValue::blank_overwrite_extent(
                            device_range.start,
                            (device_range_len / self.block_size()) as usize,
                            key_id,
                        )),
                    ),
                    AssocObj::Borrowed(self),
                );

                allocate_range.start += device_range_len;
                allocated += device_range_len;

                if transaction.mutations().len() >= MAX_TRANSACTION_SIZE {
                    self.update_allocated_size(&mut transaction, allocated, 0).await?;
                    transaction.commit_and_continue().await?;
                    allocated = 0;
                }
            }
        }

        self.update_allocated_size(&mut transaction, allocated, 0).await?;
        transaction.commit().await?;

        Ok(())
    }

    /// Return information on a contiguous set of extents that has the same allocation status,
    /// starting from `start_offset`. The information returned is if this set of extents are marked
    /// allocated/not allocated and also the size of this set (in bytes). This is used when
    /// querying slices for volumes.
    /// This function expects `start_offset` to be aligned to block size
    pub async fn is_allocated(&self, start_offset: u64) -> Result<(bool, u64), Error> {
        let block_size = self.block_size();
        assert_eq!(start_offset % block_size, 0);

        if start_offset > self.get_size() {
            bail!(FxfsError::OutOfRange)
        }

        if start_offset == self.get_size() {
            return Ok((false, 0));
        }

        let tree = &self.store().tree;
        let layer_set = tree.layer_set();
        let offset_key = ObjectKey::attribute(
            self.object_id(),
            self.attribute_id(),
            AttributeKey::Extent(Extent::search_key_from_offset(start_offset)),
        );
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullRange(&offset_key)).await?;

        let mut allocated = None;
        let mut end = start_offset;

        loop {
            // Iterate through the extents, each time setting `end` as the end of the previous
            // extent
            match iter.get() {
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data:
                                ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent_key)),
                        },
                    value: ObjectValue::Extent(extent_value),
                    ..
                }) => {
                    // Equivalent of getting no extents back
                    if *object_id != self.object_id() || *attribute_id != self.attribute_id() {
                        if allocated == Some(false) || allocated.is_none() {
                            end = self.get_size();
                            allocated = Some(false);
                        }
                        break;
                    }
                    ensure!(block_size.is_aligned(extent_key), FxfsError::Inconsistent);
                    if extent_key.start > end {
                        // If a previous extent has already been visited and we are tracking an
                        // allocated set, we are only interested in an extent where the range of the
                        // current extent follows immediately after the previous one.
                        if allocated == Some(true) {
                            break;
                        } else {
                            // The gap between the previous `end` and this extent is not allocated
                            end = extent_key.start;
                            allocated = Some(false);
                            // Continue this iteration, except now the `end` is set to the end of
                            // the "previous" extent which is this gap between the start_offset
                            // and the current extent
                        }
                    }

                    // We can assume that from here, the `end` points to the end of a previous
                    // extent.
                    match extent_value {
                        // The current extent has been allocated
                        ExtentValue::Some { .. } => {
                            // Stop searching if previous extent was marked deleted
                            if allocated == Some(false) {
                                break;
                            }
                            allocated = Some(true);
                        }
                        // This extent has been marked deleted
                        ExtentValue::None => {
                            // Stop searching if previous extent was marked allocated
                            if allocated == Some(true) {
                                break;
                            }
                            allocated = Some(false);
                        }
                    }
                    end = extent_key.end;
                }
                // This occurs when there are no extents left
                None => {
                    if allocated == Some(false) || allocated.is_none() {
                        end = self.get_size();
                        allocated = Some(false);
                    }
                    // Otherwise, we were monitoring extents that were allocated, so just exit.
                    break;
                }
                // Non-extent records (Object, Child, GraveyardEntry) are ignored.
                Some(_) => {}
            }
            iter.advance().await?;
        }

        Ok((allocated.unwrap(), end - start_offset))
    }

    pub async fn txn_write<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        offset: u64,
        buf: BufferRef<'_>,
    ) -> Result<(), Error> {
        if buf.is_empty() {
            return Ok(());
        }
        let (aligned, mut transfer_buf) = self.align_buffer(offset, buf).await?;
        self.multi_write(
            transaction,
            self.attribute_id(),
            std::slice::from_ref(&aligned),
            transfer_buf.as_mut(),
        )
        .await?;
        if offset + buf.len() as u64 > self.txn_get_size(transaction) {
            self.txn_update_size(transaction, offset + buf.len() as u64, None).await?;
        }
        Ok(())
    }

    // Writes to multiple ranges with data provided in `buf`.  The buffer can be modified in place
    // if encryption takes place.  The ranges must all be aligned and no change to content size is
    // applied; the caller is responsible for updating size if required.
    pub async fn multi_write<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        attribute_id: AttributeId,
        ranges: &[Range<u64>],
        buf: MutableBufferRef<'_>,
    ) -> Result<(), Error> {
        self.handle.multi_write(transaction, attribute_id, None, ranges, buf).await
    }

    // `buf` is mutable as an optimization, since the write may require encryption, we can
    // encrypt the buffer in-place rather than copying to another buffer if the write is
    // already aligned.
    //
    // Note: in the event of power failure during an overwrite() call, it is possible that
    // old data (which hasn't been overwritten with new bytes yet) may be exposed to the user.
    // Since the old data should be encrypted, it is probably safe to expose, although not ideal.
    pub async fn overwrite(
        &self,
        mut offset: u64,
        mut buf: MutableBufferRef<'_>,
        options: OverwriteOptions,
    ) -> Result<(), Error> {
        ensure!((buf.len() as u32) % self.store().device.block_size() == 0, FxfsError::InvalidArgs);
        let end = offset + buf.len() as u64;

        let key_id = self.get_key(None).await?.0;

        // The transaction only ends up being used if allow_allocations is true
        let mut transaction =
            if options.allow_allocations { Some(self.new_transaction().await?) } else { None };

        // We build up a list of writes to perform later
        let mut writes = FuturesOrdered::new();
        let mut first_write = options.barrier_on_first_write;

        // We create a new scope here, so that the merger iterator will get dropped before we try to
        // commit our transaction. Otherwise the transaction commit would block.
        {
            let store = self.store();
            let store_object_id = store.store_object_id;
            let allocator = store.allocator();
            let tree = &store.tree;
            let layer_set = tree.layer_set();
            let mut merger = layer_set.merger();
            let mut iter = merger
                .query(Query::FullRange(&ObjectKey::attribute(
                    self.object_id(),
                    self.attribute_id(),
                    AttributeKey::Extent(Extent::search_key_from_offset(offset)),
                )))
                .await?;
            let block_size = self.block_size();

            loop {
                let (device_offset, bytes_to_write, should_advance) = match iter.get() {
                    Some(ItemRef {
                        key:
                            ObjectKey {
                                object_id,
                                data:
                                    ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent)),
                            },
                        value: ObjectValue::Extent(ExtentValue::Some { .. }),
                        ..
                    }) if *object_id == self.object_id()
                        && *attribute_id == self.attribute_id()
                        && extent.end == offset =>
                    {
                        iter.advance().await?;
                        continue;
                    }
                    Some(ItemRef {
                        key:
                            ObjectKey {
                                object_id,
                                data:
                                    ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent)),
                            },
                        value,
                        ..
                    }) if *object_id == self.object_id()
                        && *attribute_id == self.attribute_id()
                        && extent.start <= offset =>
                    {
                        match value {
                            ObjectValue::Extent(ExtentValue::Some {
                                device_offset,
                                mode: ExtentMode::Raw,
                                ..
                            }) => {
                                ensure!(
                                    block_size.is_aligned(extent)
                                        && block_size.is_aligned(device_offset),
                                    FxfsError::Inconsistent
                                );
                                let offset_within_extent = offset - extent.start;
                                let remaining_length_of_extent = (extent
                                    .end
                                    .checked_sub(offset)
                                    .ok_or(FxfsError::Inconsistent)?)
                                    as usize;
                                // Yields (device_offset, bytes_to_write, should_advance)
                                (
                                    device_offset + offset_within_extent,
                                    min(buf.len(), remaining_length_of_extent),
                                    true,
                                )
                            }
                            ObjectValue::Extent(ExtentValue::Some { .. }) => {
                                // TODO(https://fxbug.dev/42066056): Maybe we should create
                                // a new extent without checksums?
                                bail!(
                                    "extent from ({},{}) which overlaps offset \
                                        {} has the wrong extent mode",
                                    extent.start,
                                    extent.end,
                                    offset
                                )
                            }
                            _ => {
                                bail!(
                                    "overwrite failed: extent overlapping offset {} has \
                                      unexpected ObjectValue",
                                    offset
                                )
                            }
                        }
                    }
                    maybe_item_ref => {
                        if let Some(transaction) = transaction.as_mut() {
                            assert_eq!(options.allow_allocations, true);
                            assert_eq!(offset % self.block_size(), 0);

                            // We are going to make a new extent, but let's check if there is an
                            // extent after us. If there is an extent after us, then we don't want
                            // our new extent to bump into it...
                            let mut bytes_to_allocate = self
                                .block_size()
                                .align_up(buf.len() as u64)
                                .ok_or(FxfsError::TooBig)?;
                            if let Some(ItemRef {
                                key:
                                    ObjectKey {
                                        object_id,
                                        data:
                                            ObjectKeyData::Attribute(
                                                attribute_id,
                                                AttributeKey::Extent(extent),
                                            ),
                                    },
                                ..
                            }) = maybe_item_ref
                            {
                                if *object_id == self.object_id()
                                    && *attribute_id == self.attribute_id()
                                    && offset < extent.start
                                {
                                    let bytes_until_next_extent = extent.start - offset;
                                    bytes_to_allocate =
                                        min(bytes_to_allocate, bytes_until_next_extent);
                                }
                            }

                            let device_range = allocator
                                .allocate(transaction, store_object_id, bytes_to_allocate)
                                .await?;
                            let device_range_len = device_range.end - device_range.start;
                            transaction.add(
                                store_object_id,
                                Mutation::insert_object(
                                    ObjectKey::extent(
                                        self.object_id(),
                                        self.attribute_id(),
                                        offset..offset + device_range_len,
                                    ),
                                    ObjectValue::Extent(ExtentValue::new_raw(
                                        device_range.start,
                                        key_id,
                                    )),
                                ),
                            );

                            self.update_allocated_size(transaction, device_range_len, 0).await?;

                            // Yields (device_offset, bytes_to_write, should_advance)
                            (device_range.start, min(buf.len(), device_range_len as usize), false)
                        } else {
                            bail!(
                                "no extent overlapping offset {}, \
                                and new allocations are not allowed",
                                offset
                            )
                        }
                    }
                };
                let (current_buf, remaining_buf) = buf.split_at_mut(bytes_to_write);
                let flags = if first_write {
                    first_write = false;
                    WriteFlags::PRE_BARRIER
                } else {
                    WriteFlags::empty()
                };
                writes.push_back(self.write_at(offset, current_buf, device_offset, flags));
                if remaining_buf.len() == 0 {
                    break;
                } else {
                    buf = remaining_buf;
                    offset += bytes_to_write as u64;
                    if should_advance {
                        iter.advance().await?;
                    }
                }
            }
        }

        self.store().logical_write_ops.fetch_add(1, Ordering::Relaxed);
        // The checksums are being ignored here, but we don't need to know them
        writes.try_collect::<Vec<MaybeChecksums>>().await?;

        if let Some(mut transaction) = transaction {
            assert_eq!(options.allow_allocations, true);
            if !transaction.is_empty() {
                if end > self.get_size() {
                    self.grow(&mut transaction, self.get_size(), end).await?;
                }
                transaction.commit().await?;
            }
        }

        Ok(())
    }

    // Within a transaction, the size of the object might have changed, so get the size from there
    // if it exists, otherwise, fall back on the cached size.
    fn txn_get_size(&self, transaction: &Transaction<'_>) -> u64 {
        transaction
            .get_object_mutation(
                self.store().store_object_id,
                ObjectKey::attribute(
                    self.object_id(),
                    self.attribute_id(),
                    AttributeKey::Attribute,
                ),
            )
            .and_then(|m| {
                if let ObjectItem { value: ObjectValue::Attribute { size, .. }, .. } = m.item {
                    Some(size)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| self.get_size())
    }

    pub async fn txn_update_size<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        new_size: u64,
        // Allow callers to update the has_overwrite_extents metadata if they want. If this is
        // Some it is set to the value, if None it is left unchanged.
        update_has_overwrite_extents: Option<bool>,
    ) -> Result<(), Error> {
        let key =
            ObjectKey::attribute(self.object_id(), self.attribute_id(), AttributeKey::Attribute);
        let mut mutation = if let Some(mutation) =
            transaction.get_object_mutation(self.store().store_object_id(), key.clone())
        {
            mutation.clone()
        } else {
            ObjectStoreMutation {
                item: self.store().tree().find(&key).await?.ok_or(FxfsError::NotFound)?,
                op: Operation::ReplaceOrInsert,
            }
        };
        if let ObjectValue::Attribute { size, has_overwrite_extents } = &mut mutation.item.value {
            *size = new_size;
            if let Some(update_has_overwrite_extents) = update_has_overwrite_extents {
                *has_overwrite_extents = update_has_overwrite_extents;
            }
        } else {
            bail!(anyhow!(FxfsError::Inconsistent).context("Unexpected object value"));
        }
        transaction.add_with_object(
            self.store().store_object_id(),
            Mutation::ObjectStore(mutation),
            AssocObj::Borrowed(self),
        );
        Ok(())
    }

    async fn update_allocated_size(
        &self,
        transaction: &mut Transaction<'_>,
        allocated: u64,
        deallocated: u64,
    ) -> Result<(), Error> {
        self.handle.update_allocated_size(transaction, allocated, deallocated).await
    }

    pub fn truncate_overwrite_ranges(&self, size: u64) -> Result<Option<bool>, Error> {
        let cutoff = self.block_size().align_up(size).ok_or(FxfsError::TooBig)?;
        if self.with_overwrite_ranges_mut(|ranges| ranges.map_or(false, |r| r.truncate(cutoff))) {
            // This returns true if there were ranges, but this truncate removed them all, which
            // indicates that we need to flip the has_overwrite_extents metadata flag to false.
            Ok(Some(false))
        } else {
            Ok(None)
        }
    }

    pub async fn shrink<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        size: u64,
        update_has_overwrite_extents: Option<bool>,
    ) -> Result<NeedsTrim, Error> {
        let needs_trim = self.handle.shrink(transaction, self.attribute_id(), size).await?;
        self.txn_update_size(transaction, size, update_has_overwrite_extents).await?;
        Ok(needs_trim)
    }

    pub async fn grow<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        old_size: u64,
        size: u64,
    ) -> Result<(), Error> {
        // Before growing the file, we must make sure that a previous trim has completed.
        let store = self.store();
        while matches!(
            store
                .trim_some(
                    transaction,
                    self.object_id(),
                    self.attribute_id(),
                    TrimMode::FromOffset(old_size)
                )
                .await?,
            TrimResult::Incomplete
        ) {
            transaction.commit_and_continue().await?;
        }
        // We might need to zero out the tail of the old last block.
        let block_size = self.block_size();
        if !block_size.is_aligned(old_size) {
            let layer_set = store.tree.layer_set();
            let mut merger = layer_set.merger();
            let aligned_old_size = block_size.align_down(old_size);
            let iter = merger
                .query(Query::FullRange(&ObjectKey::attribute(
                    self.object_id(),
                    self.attribute_id(),
                    AttributeKey::Extent(Extent::search_key_from_offset(aligned_old_size)),
                )))
                .await?;
            if let Some(ItemRef {
                key:
                    ObjectKey {
                        object_id,
                        data:
                            ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent_key)),
                    },
                value: ObjectValue::Extent(ExtentValue::Some { device_offset, key_id, .. }),
                ..
            }) = iter.get()
            {
                if *object_id == self.object_id() && *attribute_id == self.attribute_id() {
                    let device_offset = device_offset
                        .checked_add(aligned_old_size - extent_key.start)
                        .ok_or(FxfsError::Inconsistent)?;
                    ensure!(block_size.is_aligned(device_offset), FxfsError::Inconsistent);
                    let mut buf = self.allocate_buffer(block_size.get() as usize).await;
                    // In the case that this extent is in OverwritePartial mode, there is a
                    // possibility that the last block is allocated, but not initialized yet, in
                    // which case we don't actually need to bother zeroing out the tail. However,
                    // it's not strictly incorrect to change uninitialized data, so we skip the
                    // check and blindly do it to keep it simpler here.
                    self.read_and_decrypt(device_offset, aligned_old_size, buf.as_mut(), *key_id)
                        .await?;
                    buf.subslice_mut((old_size % block_size) as usize..buf.len()).fill(0);
                    self.multi_write(
                        transaction,
                        *attribute_id,
                        &[aligned_old_size..aligned_old_size + block_size],
                        buf.as_mut(),
                    )
                    .await?;
                }
            }
        }
        self.txn_update_size(transaction, size, None).await?;
        Ok(())
    }

    /// Attempts to pre-allocate a `file_range` of bytes for this object.
    /// Returns a set of device ranges (i.e. potentially multiple extents).
    ///
    /// It may not be possible to preallocate the entire requested range in one request
    /// due to limitations on transaction size. In such cases, we will preallocate as much as
    /// we can up to some (arbitrary, internal) limit on transaction size.
    ///
    /// `file_range.start` is modified to point at the end of the logical range
    /// that was preallocated such that repeated calls to `preallocate_range` with new
    /// transactions can be used to preallocate ranges of any size.
    ///
    /// Requested range must be a multiple of block size.
    pub async fn preallocate_range<'a>(
        &'a self,
        transaction: &mut Transaction<'a>,
        file_range: &mut Range<u64>,
    ) -> Result<Vec<Range<u64>>, Error> {
        let block_size = self.block_size();
        ensure!(block_size.is_aligned(&*file_range), FxfsError::InvalidArgs);
        ensure!(!self.handle.is_encrypted(), FxfsError::NotSupported);
        let mut ranges = Vec::new();
        let tree = &self.store().tree;
        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::attribute(
                self.object_id(),
                self.attribute_id(),
                AttributeKey::Extent(Extent::search_key_from_offset(file_range.start)),
            )))
            .await?;
        let mut allocated = 0;
        let key_id = self.get_key(None).await?.0;
        'outer: while file_range.start < file_range.end {
            let allocate_end = loop {
                match iter.get() {
                    // Case for allocated extents for the same object that overlap with file_range.
                    Some(ItemRef {
                        key:
                            ObjectKey {
                                object_id,
                                data:
                                    ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent)),
                            },
                        value: ObjectValue::Extent(ExtentValue::Some { device_offset, .. }),
                        ..
                    }) if *object_id == self.object_id()
                        && *attribute_id == self.attribute_id()
                        && extent.start < file_range.end =>
                    {
                        ensure!(
                            extent.is_valid()
                                && block_size.is_aligned(extent)
                                && block_size.is_aligned(device_offset),
                            FxfsError::Inconsistent
                        );
                        // If the start of the requested file_range overlaps with an existing extent...
                        if extent.start <= file_range.start {
                            // Record the existing extent and move on.
                            let device_range = device_offset
                                .checked_add(file_range.start - extent.start)
                                .ok_or(FxfsError::Inconsistent)?
                                ..device_offset
                                    .checked_add(min(extent.end, file_range.end) - extent.start)
                                    .ok_or(FxfsError::Inconsistent)?;
                            file_range.start += device_range.end - device_range.start;
                            ranges.push(device_range);
                            if file_range.start >= file_range.end {
                                break 'outer;
                            }
                            iter.advance().await?;
                            continue;
                        } else {
                            // There's nothing allocated between file_range.start and the beginning
                            // of this extent.
                            break extent.start;
                        }
                    }
                    // Case for deleted extents eclipsed by file_range.
                    Some(ItemRef {
                        key:
                            ObjectKey {
                                object_id,
                                data:
                                    ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent)),
                            },
                        value: ObjectValue::Extent(ExtentValue::None),
                        ..
                    }) if *object_id == self.object_id()
                        && *attribute_id == self.attribute_id()
                        && extent.end < file_range.end =>
                    {
                        iter.advance().await?;
                    }
                    _ => {
                        // We can just preallocate the rest.
                        break file_range.end;
                    }
                }
            };
            let device_range = self
                .store()
                .allocator()
                .allocate(
                    transaction,
                    self.store().store_object_id(),
                    allocate_end - file_range.start,
                )
                .await
                .context("Allocation failed")?;
            allocated += device_range.end - device_range.start;
            let this_file_range =
                file_range.start..file_range.start + device_range.end - device_range.start;
            file_range.start = this_file_range.end;
            transaction.add(
                self.store().store_object_id,
                Mutation::merge_object(
                    ObjectKey::extent(self.object_id(), self.attribute_id(), this_file_range),
                    ObjectValue::Extent(ExtentValue::new_raw(device_range.start, key_id)),
                ),
            );
            ranges.push(device_range);
            // If we didn't allocate all that we requested, we'll loop around and try again.
            // ... unless we have filled the transaction. The caller should check file_range.
            if transaction.mutations().len() > TRANSACTION_MUTATION_THRESHOLD {
                break;
            }
        }
        // Update the file size if it changed.
        if file_range.start > block_size.align_up(self.txn_get_size(transaction)).unwrap() {
            self.txn_update_size(transaction, file_range.start, None).await?;
        }
        self.update_allocated_size(transaction, allocated, 0).await?;
        Ok(ranges)
    }

    pub async fn update_attributes<'a>(
        &self,
        transaction: &mut Transaction<'a>,
        node_attributes: Option<&fio::MutableNodeAttributes>,
        change_time: Option<Timestamp>,
    ) -> Result<(), Error> {
        // This codepath is only called by files, whose wrapping key id users cannot directly set
        // as per fscrypt.
        ensure!(
            !matches!(
                node_attributes,
                Some(fio::MutableNodeAttributes { wrapping_key_id: Some(_), .. })
            ),
            FxfsError::BadPath
        );
        self.handle.update_attributes(transaction, node_attributes, change_time).await
    }

    /// Get the default set of transaction options for this object. This is mostly the overall
    /// default, modified by any [`HandleOptions`] held by this handle.
    pub fn default_transaction_options<'b>(&self) -> Options<'b> {
        self.handle.default_transaction_options()
    }

    pub async fn new_transaction<'b>(&self) -> Result<Transaction<'b>, Error> {
        self.new_transaction_with_options(self.default_transaction_options()).await
    }

    pub async fn new_transaction_with_options<'b>(
        &self,
        options: Options<'b>,
    ) -> Result<Transaction<'b>, Error> {
        self.handle.new_transaction_with_options(self.attribute_id(), options).await
    }

    /// Flushes the underlying device.  This is expensive and should be used sparingly.
    pub async fn flush_device(&self) -> Result<(), Error> {
        self.handle.flush_device().await
    }

    /// Reads an entire attribute.
    pub async fn read_attr(&self, attribute_id: AttributeId) -> Result<Option<Box<[u8]>>, Error> {
        self.handle.read_attr(attribute_id).await
    }

    /// Writes an entire attribute.  This *always* uses the volume data key.
    pub async fn write_attr(&self, attribute_id: AttributeId, data: &[u8]) -> Result<(), Error> {
        // Must be different attribute otherwise cached size gets out of date.
        assert_ne!(attribute_id, self.attribute_id());
        let store = self.store();
        let mut transaction = self.new_transaction().await?;
        if self.handle.write_attr(&mut transaction, attribute_id, data).await?.0 {
            transaction.commit_and_continue().await?;
            while matches!(
                store
                    .trim_some(
                        &mut transaction,
                        self.object_id(),
                        attribute_id,
                        TrimMode::FromOffset(data.len() as u64),
                    )
                    .await?,
                TrimResult::Incomplete
            ) {
                transaction.commit_and_continue().await?;
            }
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn read_and_decrypt(
        &self,
        device_offset: u64,
        file_offset: u64,
        buffer: MutableBufferRef<'_>,
        key_id: u64,
    ) -> Result<(), Error> {
        self.handle
            .read_and_decrypt(self.attribute_id, device_offset, file_offset, buffer, key_id)
            .await
    }

    /// Truncates a file to a given size (growing/shrinking as required).
    ///
    /// Nb: Most code will want to call truncate() instead. This method is used
    /// to update the super block -- a case where we must borrow metadata space.
    pub async fn truncate_with_options(
        &self,
        options: Options<'_>,
        size: u64,
    ) -> Result<(), Error> {
        let mut transaction = self.new_transaction_with_options(options).await?;
        {
            let state = self.state.lock();
            match &*state {
                DataObjectState::Standard(_) => {}
                _ => bail!(anyhow!(FxfsError::AccessDenied).context("Cannot truncate verity file")),
            }
        }
        let old_size = self.get_size();
        if size == old_size {
            return Ok(());
        }
        if size < old_size {
            let update_has_overwrite_ranges = self.truncate_overwrite_ranges(size)?;
            if self.shrink(&mut transaction, size, update_has_overwrite_ranges).await?.0 {
                // The file needs to be trimmed.
                transaction.commit_and_continue().await?;
                let store = self.store();
                while matches!(
                    store
                        .trim_some(
                            &mut transaction,
                            self.object_id(),
                            self.attribute_id(),
                            TrimMode::FromOffset(size)
                        )
                        .await?,
                    TrimResult::Incomplete
                ) {
                    if let Err(error) = transaction.commit_and_continue().await {
                        warn!(error:?; "Failed to trim after truncate");
                        return Ok(());
                    }
                }
                if let Err(error) = transaction.commit().await {
                    warn!(error:?; "Failed to trim after truncate");
                }
                return Ok(());
            }
        } else {
            self.grow(&mut transaction, old_size, size).await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn get_properties(&self) -> Result<ObjectProperties, Error> {
        // We don't take a read guard here since the object properties are contained in a single
        // object, which cannot be inconsistent with itself. The LSM tree does not return
        // intermediate states for a single object.
        let value = self
            .store()
            .tree
            .find_value(&ObjectKey::object(self.object_id()))
            .await?
            .expect("Unable to find object record");
        match value {
            ObjectValue::Object {
                kind: ObjectKind::File { refs, .. },
                attributes:
                    ObjectAttributes {
                        creation_time,
                        modification_time,
                        posix_attributes,
                        allocated_size,
                        access_time,
                        change_time,
                        ..
                    },
            } => Ok(ObjectProperties {
                refs,
                allocated_size,
                data_attribute_size: self.get_size(),
                creation_time,
                modification_time,
                access_time,
                change_time,
                sub_dirs: 0,
                posix_attributes,
                dir_type: DirType::Normal,
            }),
            _ => bail!(FxfsError::NotFile),
        }
    }

    // Returns the contents of this object. This object must be < |limit| bytes in size.
    pub async fn contents(&self, limit: usize) -> Result<Box<[u8]>, Error> {
        let size = self.get_size();
        if size > limit as u64 {
            bail!("Object too big ({} > {})", size, limit);
        }
        let mut buf = self.allocate_buffer(size as usize).await;
        self.read(0u64, buf.as_mut()).await?;
        Ok(buf.to_vec().into_boxed_slice())
    }

    /// Returns the set of file_offset->extent mappings for this file. The extents will be sorted by
    /// their logical offset within the file.
    ///
    /// *NOTE*: This operation is potentially expensive and should generally be avoided.
    pub async fn device_extents(&self) -> Result<Vec<FileExtent>, Error> {
        let tree = &self.store().tree;
        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let stream = self.handle.extent_stream(&mut merger, self.attribute_id()).await?;
        let extents: Vec<FileExtent> = stream.try_collect().await?;
        Ok(extents)
    }

    /// Fills |buf| with up to |buf.len()| bytes read from |offset| on the underlying device.
    /// |offset| and |buf| must both be block-aligned.
    ///
    /// This is an inherent version of the `ReadObjectHandle::read` trait method which avoids boxing
    /// the returned `Future`.
    pub async fn read(&self, offset: u64, mut buf: MutableBufferRef<'_>) -> Result<usize, Error> {
        let fs = self.store().filesystem();
        let guard = fs
            .lock_manager()
            .read_lock(lock_keys![LockKey::object_attribute(
                self.store().store_object_id,
                self.object_id(),
                self.attribute_id(),
            )])
            .await;

        let size = self.get_size();
        if offset >= size {
            return Ok(0);
        }
        let length = min(buf.len() as u64, size - offset) as usize;
        buf = buf.subslice_mut(0..length);
        self.handle.read_unchecked(self.attribute_id(), offset, buf.reborrow(), &guard).await?;
        if self.is_verified_file() {
            self.verify_data(offset as usize, buf.as_ptr_slice())?;
        }
        Ok(length)
    }
}

impl<S: HandleOwner> AssociatedObject for DataObjectHandle<S> {
    fn will_apply_mutation(&self, mutation: &Mutation, _object_id: u64, _manager: &ObjectManager) {
        match mutation {
            Mutation::ObjectStore(ObjectStoreMutation {
                item: ObjectItem { value: ObjectValue::Attribute { size, .. }, .. },
                ..
            }) => self.content_size.store(*size, atomic::Ordering::Relaxed),
            Mutation::ObjectStore(ObjectStoreMutation {
                item: ObjectItem { value: ObjectValue::VerifiedAttribute { size, .. }, .. },
                ..
            }) => {
                debug_assert_eq!(
                    self.get_size(),
                    *size,
                    "size should be set when verity is enabled and must not change"
                );
                self.finalize_fsverity_state()
            }
            Mutation::ObjectStore(ObjectStoreMutation {
                item:
                    ObjectItem {
                        key:
                            ObjectKey {
                                object_id,
                                data:
                                    ObjectKeyData::Attribute(attr_id, AttributeKey::Extent(extent)),
                            },
                        value: ObjectValue::Extent(ExtentValue::Some { mode, .. }),
                        ..
                    },
                ..
            }) if self.object_id() == *object_id && self.attribute_id() == *attr_id => match mode {
                ExtentMode::Overwrite | ExtentMode::OverwritePartial(_) => {
                    // If `enable_verity` transitioned state to `VerityStarted` concurrently while a
                    // transaction was in progress, `with_overwrite_ranges_mut` will return `None`
                    // and safely discard in-memory range updates, since verity files do not track
                    // overwrite ranges.
                    self.with_overwrite_ranges_mut(|ranges| {
                        if let Some(ranges) = ranges {
                            ranges.apply_range(extent.clone().into());
                        }
                    });
                }
                ExtentMode::Raw | ExtentMode::Cow(_) => (),
            },
            _ => {}
        }
    }
}

impl<S: HandleOwner> ObjectHandle for DataObjectHandle<S> {
    fn set_trace(&self, v: bool) {
        self.handle.set_trace(v)
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

impl<S: HandleOwner> ReadObjectHandle for DataObjectHandle<S> {
    fn read<'a, 'b, 'c>(
        &'a self,
        offset: u64,
        buf: MutableBufferRef<'b>,
    ) -> Pin<Box<dyn Future<Output = Result<usize, Error>> + Send + 'c>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(DataObjectHandle::read(self, offset, buf))
    }

    fn get_size(&self) -> u64 {
        self.content_size.load(atomic::Ordering::Relaxed)
    }
}

impl<S: HandleOwner> WriteObjectHandle for DataObjectHandle<S> {
    async fn write_or_append(&self, offset: Option<u64>, buf: BufferRef<'_>) -> Result<u64, Error> {
        let offset = offset.unwrap_or_else(|| self.get_size());
        let mut transaction = self.new_transaction().await?;
        self.txn_write(&mut transaction, offset, buf).await?;
        let new_size = self.txn_get_size(&transaction);
        transaction.commit().await?;
        Ok(new_size)
    }

    async fn truncate(&self, size: u64) -> Result<(), Error> {
        self.truncate_with_options(self.default_transaction_options(), size).await
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// Like object_handle::Writer, but allows custom transaction options to be set, and makes every
/// write go directly to the handle in a transaction.
pub struct DirectWriter<'a, S: HandleOwner> {
    handle: &'a DataObjectHandle<S>,
    options: transaction::Options<'a>,
    buffer: Buffer<'a>,
    offset: u64,
    buf_offset: usize,
}

const BUFFER_SIZE: usize = 1_048_576;

impl<S: HandleOwner> Drop for DirectWriter<'_, S> {
    fn drop(&mut self) {
        if self.buf_offset != 0 {
            warn!("DirectWriter: dropping data, did you forget to call complete?");
        }
    }
}

impl<'a, S: HandleOwner> DirectWriter<'a, S> {
    pub async fn new(
        handle: &'a DataObjectHandle<S>,
        options: transaction::Options<'a>,
    ) -> DirectWriter<'a, S> {
        Self {
            handle,
            options,
            buffer: handle.allocate_buffer(BUFFER_SIZE).await,
            offset: 0,
            buf_offset: 0,
        }
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let mut transaction = self.handle.new_transaction_with_options(self.options).await?;
        self.handle
            .txn_write(&mut transaction, self.offset, self.buffer.subslice(0..self.buf_offset))
            .await?;
        transaction.commit().await?;
        self.offset += self.buf_offset as u64;
        self.buf_offset = 0;
        Ok(())
    }
}

impl<'a, S: HandleOwner> WriteBytes for DirectWriter<'a, S> {
    fn block_size(&self) -> BlockSize {
        self.handle.block_size()
    }

    async fn write_bytes(&mut self, mut buf: &[u8]) -> Result<(), Error> {
        while buf.len() > 0 {
            let to_do = std::cmp::min(buf.len(), BUFFER_SIZE - self.buf_offset);
            self.buffer
                .subslice_mut(self.buf_offset..self.buf_offset + to_do)
                .copy_from_slice(&buf[..to_do]);
            self.buf_offset += to_do;
            if self.buf_offset == BUFFER_SIZE {
                self.flush().await?;
            }
            buf = &buf[to_do..];
        }
        Ok(())
    }

    async fn complete(mut self) -> Result<u64, Error> {
        self.flush().await?;
        Ok(self.offset + self.buf_offset as u64)
    }

    async fn skip(&mut self, amount: u64) -> Result<(), Error> {
        if (BUFFER_SIZE - self.buf_offset) as u64 > amount {
            self.buffer.subslice_mut(self.buf_offset..self.buf_offset + amount as usize).fill(0);
            self.buf_offset += amount as usize;
        } else {
            self.flush().await?;
            self.offset += amount;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::errors::FxfsError;
    use crate::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
    use crate::fsck::{
        FsckOptions, fsck, fsck_volume, fsck_volume_with_options, fsck_with_options,
    };
    use crate::lsm_tree::Query;
    use crate::lsm_tree::types::{ItemRef, LayerIterator};
    use crate::object_handle::{
        ObjectHandle, ObjectProperties, ReadObjectHandle, WriteObjectHandle,
    };
    use crate::object_store::data_object_handle::{OverwriteOptions, WRITE_ATTR_BATCH_SIZE};
    use crate::object_store::directory::replace_child;
    use crate::object_store::object_record::{FsverityMetadata, ObjectKey, ObjectValue, Timestamp};
    use crate::object_store::transaction::{Mutation, Options, lock_keys};
    use crate::object_store::volume::root_volume;
    use crate::object_store::{
        AttributeId, AttributeKey, DataObjectHandle, DirType, Directory, Extent, ExtentMode,
        ExtentValue, HandleOptions, LockKey, NewChildStoreOptions, ObjectKeyData, ObjectStore,
        PosixAttributes, StoreOptions, TRANSACTION_MUTATION_THRESHOLD,
    };
    use crate::range::RangeExt;
    use crate::round::{round_down, round_up};
    use assert_matches::assert_matches;
    use bit_vec::BitVec;
    use fidl_fuchsia_io as fio;
    use fsverity_merkle::{FsVerityDescriptor, FsVerityDescriptorRaw};
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::FutureExt;
    use futures::channel::oneshot::channel;
    use futures::stream::{FuturesUnordered, StreamExt};
    use fxfs_crypto::{Crypt, EncryptionKey, KeyPurpose};
    use fxfs_insecure_crypto::new_insecure_crypt;
    use std::ops::Range;
    use std::sync::Arc;
    use std::time::Duration;
    use storage_device::DeviceHolder;
    use storage_device::fake_device::FakeDevice;

    const TEST_DEVICE_BLOCK_SIZE: u32 = 512;

    // Some tests (the preallocate_range ones) currently assume that the data only occupies a single
    // device block.
    const TEST_DATA_OFFSET: u64 = 5000;
    const TEST_DATA: &[u8] = b"hello";
    const TEST_OBJECT_SIZE: u64 = 5678;
    const TEST_OBJECT_ALLOCATED_SIZE: u64 = 4096;
    const TEST_OBJECT_NAME: &str = "foo";

    async fn test_filesystem() -> OpenFxFilesystem {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        FxFilesystem::new_empty(device).await.expect("new_empty failed")
    }

    async fn create_object_with_key(
        fs: Arc<FxFilesystem>,
        crypt: Option<&dyn Crypt>,
        write_object_test_data: bool,
    ) -> DataObjectHandle<ObjectStore> {
        let store = fs.root_store();
        let object;

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

        object = if let Some(crypt) = crypt {
            let object_id = store.get_next_object_id().await.unwrap();
            let (key, unwrapped_key) =
                crypt.create_key(object_id.get(), KeyPurpose::Data).await.unwrap();
            ObjectStore::create_object_with_key(
                &store,
                &mut transaction,
                object_id,
                HandleOptions::default(),
                EncryptionKey::Fxfs(key),
                unwrapped_key,
            )
            .await
            .expect("create_object failed")
        } else {
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed")
        };

        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");
        root_directory
            .add_child_file(&mut transaction, TEST_OBJECT_NAME, &object)
            .await
            .expect("add_child_file failed");

        if write_object_test_data {
            let align = TEST_DATA_OFFSET as usize % TEST_DEVICE_BLOCK_SIZE as usize;
            let mut buf = object.allocate_buffer(align + TEST_DATA.len()).await;
            buf.subslice_mut(align..buf.len()).copy_from_slice(TEST_DATA);
            object
                .txn_write(&mut transaction, TEST_DATA_OFFSET, buf.subslice(align..buf.len()))
                .await
                .expect("write failed");
        }
        transaction.commit().await.expect("commit failed");
        object.truncate(TEST_OBJECT_SIZE).await.expect("truncate failed");
        object
    }

    async fn test_filesystem_and_object_with_key(
        crypt: Option<&dyn Crypt>,
        write_object_test_data: bool,
    ) -> (OpenFxFilesystem, DataObjectHandle<ObjectStore>) {
        let fs = test_filesystem().await;
        let object = create_object_with_key(fs.clone(), crypt, write_object_test_data).await;
        (fs, object)
    }

    async fn test_filesystem_and_object() -> (OpenFxFilesystem, DataObjectHandle<ObjectStore>) {
        test_filesystem_and_object_with_key(Some(&new_insecure_crypt()), true).await
    }

    async fn test_filesystem_and_empty_object() -> (OpenFxFilesystem, DataObjectHandle<ObjectStore>)
    {
        test_filesystem_and_object_with_key(Some(&new_insecure_crypt()), false).await
    }

    #[fuchsia::test]
    async fn test_zero_buf_len_read() {
        let (fs, object) = test_filesystem_and_object().await;
        let mut buf = object.allocate_buffer(0).await;
        assert_eq!(object.read(0u64, buf.as_mut()).await.expect("read failed"), 0);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_beyond_eof_read() {
        let (fs, object) = test_filesystem_and_object().await;
        let offset = TEST_OBJECT_SIZE as usize - 2;
        let align = (offset as u64 % fs.block_size()) as usize;
        let len: usize = 2;
        let mut buf = object.allocate_buffer(align + len + 1).await;
        buf.fill(123u8);
        assert_eq!(
            object.read((offset - align) as u64, buf.as_mut()).await.expect("read failed"),
            align + len
        );
        assert_eq!(&buf.as_ptr_slice().subslice(align..align + len).to_vec()[..], &vec![0u8; len]);
        assert_eq!(
            &buf.as_ptr_slice().subslice(align + len..buf.len()).to_vec()[..],
            &vec![123u8; buf.len() - align - len]
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_beyond_eof_read_from() {
        let (fs, object) = test_filesystem_and_object().await;
        let handle = &*object;
        let offset = TEST_OBJECT_SIZE as usize - 2;
        let align = (offset as u64 % fs.block_size()) as usize;
        let len: usize = 2;
        let mut buf = object.allocate_buffer(align + len + 1).await;
        buf.fill(123u8);
        assert_eq!(
            handle
                .read(AttributeId::DATA, (offset - align) as u64, buf.as_mut())
                .await
                .expect("read failed"),
            align + len
        );
        assert_eq!(&buf.as_ptr_slice().subslice(align..align + len).to_vec()[..], &vec![0u8; len]);
        assert_eq!(
            &buf.as_ptr_slice().subslice(align + len..buf.len()).to_vec()[..],
            &vec![123u8; buf.len() - align - len]
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_beyond_eof_read_unchecked() {
        let (fs, object) = test_filesystem_and_object().await;
        let offset = TEST_OBJECT_SIZE as usize - 2;
        let align = (offset as u64 % fs.block_size()) as usize;
        let len: usize = 2;
        let mut buf = object.allocate_buffer(align + len + 1).await;
        buf.fill(123u8);
        let guard = fs
            .lock_manager()
            .read_lock(lock_keys![LockKey::object_attribute(
                object.store().store_object_id,
                object.object_id(),
                AttributeId::DATA,
            )])
            .await;
        object
            .read_unchecked(AttributeId::DATA, (offset - align) as u64, buf.as_mut(), &guard)
            .await
            .expect("read failed");
        assert_eq!(
            &buf.as_ptr_slice().subslice(align..buf.len()).to_vec()[..],
            &vec![0u8; len + 1]
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_sparse() {
        let (fs, object) = test_filesystem_and_object().await;
        // Deliberately read not right to eof.
        let len = TEST_OBJECT_SIZE as usize - 1;
        let mut buf = object.allocate_buffer(len).await;
        buf.fill(123u8);
        assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed"), len);
        let mut expected = vec![0; len];
        let offset = TEST_DATA_OFFSET as usize;
        expected[offset..offset + TEST_DATA.len()].copy_from_slice(TEST_DATA);
        assert_eq!(&buf.as_ptr_slice().subslice(0..len).to_vec()[..], &expected[..]);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_after_writes_interspersed_with_flush() {
        let (fs, object) = test_filesystem_and_object().await;

        object.owner().flush().await.expect("flush failed");

        // Write more test data to the first block fo the file.
        let mut buf = object.allocate_buffer(TEST_DATA.len()).await;
        buf.copy_from_slice(TEST_DATA);
        object.write_or_append(Some(0u64), buf.as_ref()).await.expect("write failed");

        let len = TEST_OBJECT_SIZE as usize - 1;
        let mut buf = object.allocate_buffer(len).await;
        buf.fill(123u8);
        assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed"), len);

        let mut expected = vec![0u8; len];
        let offset = TEST_DATA_OFFSET as usize;
        expected[offset..offset + TEST_DATA.len()].copy_from_slice(TEST_DATA);
        expected[..TEST_DATA.len()].copy_from_slice(TEST_DATA);
        assert_eq!(&buf.to_vec(), &expected);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_after_truncate_and_extend() {
        let (fs, object) = test_filesystem_and_object().await;

        // Arrange for there to be <extent><deleted-extent><extent>.
        let mut buf = object.allocate_buffer(TEST_DATA.len()).await;
        buf.copy_from_slice(TEST_DATA);
        // This adds an extent at 0..512.
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        // This deletes 512..1024.
        object.truncate(3).await.expect("truncate failed");
        let data = b"foo";
        let offset = 1500u64;
        let align = (offset % fs.block_size()) as usize;
        let mut buf = object.allocate_buffer(align + data.len()).await;
        buf.subslice_mut(align..buf.len()).copy_from_slice(data);
        // This adds 1024..1536.
        object
            .write_or_append(Some(1500), buf.subslice(align..buf.len()))
            .await
            .expect("write failed");

        const LEN1: usize = 1503;
        let mut buf = object.allocate_buffer(LEN1).await;
        buf.fill(123u8);
        assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed"), LEN1);
        let mut expected = [0; LEN1];
        expected[..3].copy_from_slice(&TEST_DATA[..3]);
        expected[1500..].copy_from_slice(b"foo");
        assert_eq!(&buf.to_vec(), &expected);

        // Also test a read that ends midway through the deleted extent.
        const LEN2: usize = 601;
        let mut buf = object.allocate_buffer(LEN2).await;
        buf.fill(123u8);
        assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed"), LEN2);
        assert_eq!(buf.to_vec(), &expected[..LEN2]);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_read_whole_blocks_with_multiple_objects() {
        let (fs, object) = test_filesystem_and_object().await;
        let block_size = object.block_size().get() as usize;
        let mut buffer = object.allocate_buffer(block_size).await;
        buffer.fill(0xaf);
        object.write_or_append(Some(0), buffer.as_ref()).await.expect("write failed");

        let store = object.owner();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let object2 =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");
        transaction.commit().await.expect("commit failed");
        let mut ef_buffer = object.allocate_buffer(block_size).await;
        ef_buffer.fill(0xef);
        object2.write_or_append(Some(0), ef_buffer.as_ref()).await.expect("write failed");

        let mut buffer = object.allocate_buffer(block_size).await;
        buffer.fill(0xaf);
        object
            .write_or_append(Some(block_size as u64), buffer.as_ref())
            .await
            .expect("write failed");
        object.truncate(3 * block_size as u64).await.expect("truncate failed");
        object2
            .write_or_append(Some(block_size as u64), ef_buffer.as_ref())
            .await
            .expect("write failed");

        let mut buffer = object.allocate_buffer(4 * block_size).await;
        buffer.fill(123);
        assert_eq!(object.read(0, buffer.as_mut()).await.expect("read failed"), 3 * block_size);
        assert_eq!(
            &buffer.as_ptr_slice().subslice(0..2 * block_size).to_vec()[..],
            &vec![0xaf; 2 * block_size]
        );
        assert_eq!(
            &buffer.as_ptr_slice().subslice(2 * block_size..3 * block_size).to_vec()[..],
            &vec![0; block_size]
        );
        assert_eq!(object2.read(0, buffer.as_mut()).await.expect("read failed"), 2 * block_size);
        assert_eq!(
            &buffer.as_ptr_slice().subslice(0..2 * block_size).to_vec()[..],
            &vec![0xef; 2 * block_size]
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_alignment() {
        let (fs, object) = test_filesystem_and_object().await;

        struct AlignTest {
            fill: u8,
            object: DataObjectHandle<ObjectStore>,
            mirror: Vec<u8>,
        }

        impl AlignTest {
            async fn new(object: DataObjectHandle<ObjectStore>) -> Self {
                let mirror = {
                    let mut buf = object.allocate_buffer(object.get_size() as usize).await;
                    assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed"), buf.len());
                    buf.to_vec()
                };
                Self { fill: 0, object, mirror }
            }

            // Fills |range| of self.object with a byte value (self.fill) and mirrors the same
            // operation to an in-memory copy of the object.
            // Each subsequent call bumps the value of fill.
            // It is expected that the object and its mirror maintain identical content.
            async fn test(&mut self, range: Range<u64>) {
                let mut buf = self.object.allocate_buffer((range.end - range.start) as usize).await;
                self.fill += 1;
                buf.fill(self.fill);
                self.object
                    .write_or_append(Some(range.start), buf.as_ref())
                    .await
                    .expect("write_or_append failed");
                if range.end > self.mirror.len() as u64 {
                    self.mirror.resize(range.end as usize, 0);
                }
                self.mirror[range.start as usize..range.end as usize].fill(self.fill);
                let mut buf = self.object.allocate_buffer(self.mirror.len() + 1).await;
                assert_eq!(
                    self.object.read(0, buf.as_mut()).await.expect("read failed"),
                    self.mirror.len()
                );
                assert_eq!(
                    &buf.as_ptr_slice().subslice(0..self.mirror.len()).to_vec()[..],
                    self.mirror.as_slice()
                );
            }
        }

        let block_size = object.block_size().get();
        let mut align = AlignTest::new(object).await;

        // Fill the object to start with (with 1).
        align.test(0..2 * block_size + 1).await;

        // Unaligned head (fills with 2, overwrites that with 3).
        align.test(1..block_size).await;
        align.test(1..2 * block_size).await;

        // Unaligned tail (fills with 4 and 5).
        align.test(0..block_size - 1).await;
        align.test(0..2 * block_size - 1).await;

        // Both unaligned (fills with 6 and 7).
        align.test(1..block_size - 1).await;
        align.test(1..2 * block_size - 1).await;

        fs.close().await.expect("Close failed");
    }

    async fn test_preallocate_common(fs: &FxFilesystem, object: DataObjectHandle<ObjectStore>) {
        let allocator = fs.allocator();
        let allocated_before = allocator.get_allocated_bytes();
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        object
            .preallocate_range(&mut transaction, &mut (0..fs.block_size().get()))
            .await
            .expect("preallocate_range failed");
        transaction.commit().await.expect("commit failed");
        assert!(object.get_size() < 1048576);
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        object
            .preallocate_range(&mut transaction, &mut (0..1048576))
            .await
            .expect("preallocate_range failed");
        transaction.commit().await.expect("commit failed");
        assert_eq!(object.get_size(), 1048576);
        // Check that it didn't reallocate the space for the existing extent
        let allocated_after = allocator.get_allocated_bytes();
        assert_eq!(allocated_after - allocated_before, 1048576 - fs.block_size());

        let mut buf = object
            .allocate_buffer(fs.block_size().align_up(TEST_DATA_OFFSET).unwrap() as usize)
            .await;
        buf.fill(47);
        object
            .write_or_append(Some(0), buf.subslice(0..TEST_DATA_OFFSET as usize))
            .await
            .expect("write failed");
        buf.fill(95);
        let offset = fs.block_size().align_up(TEST_OBJECT_SIZE).unwrap();
        object
            .overwrite(offset, buf.as_mut(), OverwriteOptions::default())
            .await
            .expect("write failed");

        // Make sure there were no more allocations.
        assert_eq!(allocator.get_allocated_bytes(), allocated_after);

        // Read back the data and make sure it is what we expect.
        let mut buf = object.allocate_buffer(104876).await;
        assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed"), buf.len());
        assert_eq!(
            &buf.as_ptr_slice().subslice(0..TEST_DATA_OFFSET as usize).to_vec()[..],
            &[47; TEST_DATA_OFFSET as usize]
        );
        assert_eq!(
            &buf.as_ptr_slice()
                .subslice(TEST_DATA_OFFSET as usize..TEST_DATA_OFFSET as usize + TEST_DATA.len())
                .to_vec()[..],
            TEST_DATA
        );
        assert_eq!(
            &buf.as_ptr_slice().subslice(offset as usize..offset as usize + 2048).to_vec()[..],
            &[95; 2048]
        );
    }

    #[fuchsia::test]
    async fn test_unaligned_overwrite_returns_error() {
        let (fs, object) = test_filesystem_and_object().await;
        let mut buf = object.allocate_buffer(100).await;
        let res = object.overwrite(0, buf.as_mut(), OverwriteOptions::default()).await;
        assert!(matches!(res, Err(e) if FxfsError::InvalidArgs.matches(&e)));
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_unaligned_preallocate_returns_error() {
        let (fs, object) = test_filesystem_and_object().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let res = object.preallocate_range(&mut transaction, &mut (0..100)).await;
        assert!(matches!(res, Err(e) if FxfsError::InvalidArgs.matches(&e)));
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_encrypted_preallocate_returns_error() {
        let (fs, object) = test_filesystem_and_object().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let bs = fs.block_size();
        let res = object.preallocate_range(&mut transaction, &mut (0..bs.get())).await;
        assert!(matches!(res, Err(e) if FxfsError::NotSupported.matches(&e)));
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_preallocate_range() {
        let (fs, object) = test_filesystem_and_object_with_key(None, true).await;
        test_preallocate_common(&fs, object).await;
        fs.close().await.expect("Close failed");
    }

    // This is identical to the previous test except that we flush so that extents end up in
    // different layers.
    #[fuchsia::test]
    async fn test_preallocate_succeeds_when_extents_are_in_different_layers() {
        let (fs, object) = test_filesystem_and_object_with_key(None, true).await;
        object.owner().flush().await.expect("flush failed");
        test_preallocate_common(&fs, object).await;
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_already_preallocated() {
        let (fs, object) = test_filesystem_and_object_with_key(None, true).await;
        let allocator = fs.allocator();
        let allocated_before = allocator.get_allocated_bytes();
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        let offset = fs.block_size().align_down(TEST_DATA_OFFSET);
        object
            .preallocate_range(&mut transaction, &mut (offset..offset + fs.block_size()))
            .await
            .expect("preallocate_range failed");
        transaction.commit().await.expect("commit failed");
        // Check that it didn't reallocate any new space.
        assert_eq!(allocator.get_allocated_bytes(), allocated_before);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_overwrite_when_preallocated_at_start_of_file() {
        // The standard test data we put in the test object would cause an extent with checksums
        // to be created, which overwrite() doesn't support. So we create an empty object instead.
        let (fs, object) = test_filesystem_and_empty_object().await;

        let object = ObjectStore::open_object(
            object.owner(),
            object.object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open_object failed");

        assert_eq!(fs.block_size(), 4096);

        let mut write_buf = object.allocate_buffer(4096).await;
        write_buf.fill(95);

        // First try to overwrite without allowing allocations
        // We expect this to fail, since nothing is allocated yet
        object
            .overwrite(0, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");

        // Now preallocate some space (exactly one block)
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        object
            .preallocate_range(&mut transaction, &mut (0..4096 as u64))
            .await
            .expect("preallocate_range failed");
        transaction.commit().await.expect("commit failed");

        // Now try the same overwrite command as before, it should work this time,
        // even with allocations disabled...
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(0, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[0; 4096]);
        }
        object
            .overwrite(0, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(0, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }

        // Now try to overwrite at offset 4096. We expect this to fail, since we only preallocated
        // one block earlier at offset 0
        object
            .overwrite(4096, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");

        // We can't assert anything about the existing bytes, because they haven't been allocated
        // yet and they could contain any values
        object
            .overwrite(
                4096,
                write_buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(4096, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }

        // Check that the overwrites haven't messed up the filesystem state
        let fsck_options = FsckOptions {
            fail_on_warning: true,
            no_lock: true,
            on_error: Box::new(|err| println!("fsck error: {:?}", err)),
            ..Default::default()
        };
        fsck_with_options(fs.clone(), &fsck_options).await.expect("fsck failed");

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_overwrite_large_buffer_and_file_with_many_holes() {
        // The standard test data we put in the test object would cause an extent with checksums
        // to be created, which overwrite() doesn't support. So we create an empty object instead.
        let (fs, object) = test_filesystem_and_empty_object().await;

        let object = ObjectStore::open_object(
            object.owner(),
            object.object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open_object failed");

        assert_eq!(fs.block_size(), 4096);
        assert_eq!(object.get_size(), TEST_OBJECT_SIZE);

        // Let's create some non-holes
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        object
            .preallocate_range(&mut transaction, &mut (4096..8192 as u64))
            .await
            .expect("preallocate_range failed");
        object
            .preallocate_range(&mut transaction, &mut (16384..32768 as u64))
            .await
            .expect("preallocate_range failed");
        object
            .preallocate_range(&mut transaction, &mut (65536..131072 as u64))
            .await
            .expect("preallocate_range failed");
        object
            .preallocate_range(&mut transaction, &mut (262144..524288 as u64))
            .await
            .expect("preallocate_range failed");
        transaction.commit().await.expect("commit failed");

        assert_eq!(object.get_size(), 524288);

        let mut write_buf = object.allocate_buffer(4096).await;
        write_buf.fill(95);

        // We shouldn't be able to overwrite in the holes if new allocations aren't enabled
        object
            .overwrite(0, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");
        object
            .overwrite(8192, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");
        object
            .overwrite(32768, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");
        object
            .overwrite(131072, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");

        // But we should be able to overwrite in the prealloc'd areas without needing allocations
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(4096, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[0; 4096]);
        }
        object
            .overwrite(4096, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(4096, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(16384, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[0; 4096]);
        }
        object
            .overwrite(16384, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(16384, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(65536, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[0; 4096]);
        }
        object
            .overwrite(65536, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(65536, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(262144, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[0; 4096]);
        }
        object
            .overwrite(262144, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(262144, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }

        // Now let's try to do a huge overwrite, that spans over many holes and non-holes
        let mut huge_write_buf = object.allocate_buffer(524288).await;
        huge_write_buf.fill(96);

        // With allocations disabled, the big overwrite should fail...
        object
            .overwrite(0, huge_write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");
        // ... but it should work when allocations are enabled
        object
            .overwrite(
                0,
                huge_write_buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(524288).await;
            object.read(0, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[96; 524288]);
        }

        // Check that the overwrites haven't messed up the filesystem state
        let fsck_options = FsckOptions {
            fail_on_warning: true,
            no_lock: true,
            on_error: Box::new(|err| println!("fsck error: {:?}", err)),
            ..Default::default()
        };
        fsck_with_options(fs.clone(), &fsck_options).await.expect("fsck failed");

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_overwrite_when_unallocated_at_start_of_file() {
        // The standard test data we put in the test object would cause an extent with checksums
        // to be created, which overwrite() doesn't support. So we create an empty object instead.
        let (fs, object) = test_filesystem_and_empty_object().await;

        let object = ObjectStore::open_object(
            object.owner(),
            object.object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open_object failed");

        assert_eq!(fs.block_size(), 4096);

        let mut write_buf = object.allocate_buffer(4096).await;
        write_buf.fill(95);

        // First try to overwrite without allowing allocations
        // We expect this to fail, since nothing is allocated yet
        object
            .overwrite(0, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");

        // Now try the same overwrite command as before, but allow allocations
        object
            .overwrite(
                0,
                write_buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(0, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }

        // Now try to overwrite at the next block. This should fail if allocations are disabled
        object
            .overwrite(4096, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");

        // ... but it should work if allocations are enabled
        object
            .overwrite(
                4096,
                write_buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(4096, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }

        // Check that the overwrites haven't messed up the filesystem state
        let fsck_options = FsckOptions {
            fail_on_warning: true,
            no_lock: true,
            on_error: Box::new(|err| println!("fsck error: {:?}", err)),
            ..Default::default()
        };
        fsck_with_options(fs.clone(), &fsck_options).await.expect("fsck failed");

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_overwrite_can_extend_a_file() {
        // The standard test data we put in the test object would cause an extent with checksums
        // to be created, which overwrite() doesn't support. So we create an empty object instead.
        let (fs, object) = test_filesystem_and_empty_object().await;

        let object = ObjectStore::open_object(
            object.owner(),
            object.object_id(),
            HandleOptions::default(),
            None,
        )
        .await
        .expect("open_object failed");

        assert_eq!(fs.block_size(), 4096);
        assert_eq!(object.get_size(), TEST_OBJECT_SIZE);

        let mut write_buf = object.allocate_buffer(4096).await;
        write_buf.fill(95);

        // Let's try to fill up the last block, and increase the file size in doing so
        let last_block_offset = round_down(TEST_OBJECT_SIZE, 4096 as u32);

        // Expected to fail with allocations disabled
        object
            .overwrite(last_block_offset, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");
        // ... but expected to succeed with allocations enabled
        object
            .overwrite(
                last_block_offset,
                write_buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(last_block_offset, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }

        assert_eq!(object.get_size(), 8192);

        // Let's try to write at the next block, too
        let next_block_offset = round_up(TEST_OBJECT_SIZE, 4096 as u32).unwrap();

        // Expected to fail with allocations disabled
        object
            .overwrite(next_block_offset, write_buf.as_mut(), OverwriteOptions::default())
            .await
            .expect_err("overwrite succeeded");
        // ... but expected to succeed with allocations enabled
        object
            .overwrite(
                next_block_offset,
                write_buf.as_mut(),
                OverwriteOptions { allow_allocations: true, ..Default::default() },
            )
            .await
            .expect("overwrite failed");
        {
            let mut read_buf = object.allocate_buffer(4096).await;
            object.read(next_block_offset, read_buf.as_mut()).await.expect("read failed");
            assert_eq!(&read_buf.to_vec()[..], &[95; 4096]);
        }

        assert_eq!(object.get_size(), 12288);

        // Check that the overwrites haven't messed up the filesystem state
        let fsck_options = FsckOptions {
            fail_on_warning: true,
            no_lock: true,
            on_error: Box::new(|err| println!("fsck error: {:?}", err)),
            ..Default::default()
        };
        fsck_with_options(fs.clone(), &fsck_options).await.expect("fsck failed");

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_enable_verity() {
        let fs: OpenFxFilesystem = test_filesystem().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        let object = Arc::new(
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed"),
        );

        transaction.commit().await.unwrap();

        object
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("set verified file metadata failed");

        let handle =
            ObjectStore::open_object(&store, object.object_id(), HandleOptions::default(), None)
                .await
                .expect("open_object failed");

        assert!(handle.is_verified_file());

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_enable_verity_large_file() {
        // Need to make a large FakeDevice to create space for a 67 MB file.
        let device = DeviceHolder::new(FakeDevice::new(262144, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        let handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("failed to create object");
        transaction.commit().await.expect("commit failed");
        let mut offset = 0;

        // Write a file big enough to trigger multiple transactions on enable_verity().
        let mut buf = handle.allocate_buffer(WRITE_ATTR_BATCH_SIZE).await;
        buf.fill(1);
        for _ in 0..130 {
            handle.write_or_append(Some(offset), buf.as_ref()).await.expect("write failed");
            offset += WRITE_ATTR_BATCH_SIZE as u64;
        }

        handle
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("set verified file metadata failed");

        let mut buf = handle.allocate_buffer(WRITE_ATTR_BATCH_SIZE).await;
        offset = 0;
        for _ in 0..130 {
            handle.read(offset, buf.as_mut()).await.expect("verification during read should fail");
            assert_eq!(buf.to_vec(), &[1; WRITE_ATTR_BATCH_SIZE]);
            offset += WRITE_ATTR_BATCH_SIZE as u64;
        }

        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_retry_enable_verity_on_reboot() {
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let root_store = fs.root_store();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");

        let handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions::default(),
            None,
        )
        .await
        .expect("failed to create object");
        transaction.commit().await.expect("commit failed");

        let object_id = {
            let mut transaction = handle.new_transaction().await.expect("new_transaction failed");
            transaction.add(
                root_store.store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::graveyard_attribute_entry(
                        root_store.graveyard_directory_object_id(),
                        handle.object_id(),
                        AttributeId::FSVERITY_MERKLE,
                    ),
                    ObjectValue::Some,
                ),
            );

            // This write should span three transactions. This test mimics the behavior when the
            // last transaction gets interrupted by a filesystem.close().
            handle
                .write_new_attr_in_batches(
                    &mut transaction,
                    AttributeId::FSVERITY_MERKLE,
                    &vec![0; 2 * WRITE_ATTR_BATCH_SIZE],
                    WRITE_ATTR_BATCH_SIZE,
                )
                .await
                .expect("failed to write merkle attribute");

            handle.object_id()
            // Drop the transaction to simulate interrupting the merkle tree creation as well as to
            // release the transaction locks.
        };

        fs.close().await.expect("failed to close filesystem");
        let device = fs.take_device().await;
        device.reopen(false);

        let fs =
            FxFilesystemBuilder::new().read_only(true).open(device).await.expect("open failed");
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("failed to close filesystem");
        let device = fs.take_device().await;
        device.reopen(false);

        // On open, the filesystem will call initial_reap which will call queue_tombstone().
        let fs = FxFilesystem::open(device).await.expect("open failed");
        let root_store = fs.root_store();
        let handle =
            ObjectStore::open_object(&root_store, object_id, HandleOptions::default(), None)
                .await
                .expect("open_object failed");
        handle
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("set verified file metadata failed");

        // `flush` will ensure that initial reap fully processes all the graveyard entries. This
        // isn't strictly necessary for the test to pass (the graveyard marker was already
        // processed during `enable_verity`), but it does help catch bugs, such as the attribute
        // graveyard entry not being removed upon processing.
        fs.graveyard().flush().await;
        let merkle_data = handle
            .read_attr(AttributeId::FSVERITY_MERKLE)
            .await
            .expect("read_attr failed")
            .expect("No attr found");
        assert!(
            FsVerityDescriptor::new(&merkle_data[..], handle.block_size().get() as usize).is_ok()
        );
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_verify_data_corrupt_file() {
        let fs: OpenFxFilesystem = test_filesystem().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        let object = Arc::new(
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed"),
        );

        transaction.commit().await.unwrap();

        let mut buf = object.allocate_buffer(5 * fs.block_size().get() as usize).await;
        buf.fill(123);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        object
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("set verified file metadata failed");

        // Change file contents and ensure verification fails
        buf.fill(234);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        object.read(0, buf.as_mut()).await.expect_err("verification during read should fail");

        fs.close().await.expect("Close failed");
    }

    // TODO(https://fxbug.dev/450398331): More tests to be added when this can support writing the
    // f2fs format natively. For now, relying on tests inside of the f2fs_reader to exercise more
    // paths.
    #[fuchsia::test]
    async fn test_parse_f2fs_verity() {
        let fs: OpenFxFilesystem = test_filesystem().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        let object = Arc::new(
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed"),
        );

        transaction.commit().await.unwrap();
        let file_size = fs.block_size() * 2;
        // Write over one block to make there be leaf hashes.
        {
            let mut buf = object.allocate_buffer(file_size as usize).await;
            buf.fill(64);
            assert_eq!(
                object.write_or_append(None, buf.as_ref()).await.expect("Writing to file."),
                file_size
            );
        }

        // Enable verity normally, then shift the type.
        object
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("set verified file metadata failed");
        let (verity_info, root_hash) = object.get_descriptor().unwrap();

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::Object {
                    store_object_id: store.store_object_id(),
                    object_id: object.object_id()
                }],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    object.object_id(),
                    AttributeId::DATA,
                    AttributeKey::Attribute,
                ),
                ObjectValue::verified_attribute(
                    file_size,
                    FsverityMetadata::F2fs(0..(fs.block_size() * 2)),
                ),
            ),
        );
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    object.object_id(),
                    AttributeId::FSVERITY_MERKLE,
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(fs.block_size() * 2, false),
            ),
        );
        {
            let descriptor = FsVerityDescriptorRaw::new(
                fio::HashAlgorithm::Sha256,
                fs.block_size().get(),
                file_size,
                root_hash.as_slice(),
                match &verity_info.salt {
                    Some(salt) => salt.as_slice(),
                    None => [0u8; 0].as_slice(),
                },
            )
            .expect("Creating descriptor");
            let mut buf = object.allocate_buffer(fs.block_size().get() as usize).await;
            let mut temp = vec![0u8; fs.block_size().get() as usize];
            descriptor.write_to_slice(&mut temp).expect("Writing descriptor to buf");
            buf.copy_from_slice(&temp);
            object
                .multi_write(
                    &mut transaction,
                    AttributeId::FSVERITY_MERKLE,
                    &[fs.block_size().get()..(fs.block_size() * 2)],
                    buf.as_mut(),
                )
                .await
                .expect("Writing descriptor");
        }
        transaction.commit().await.unwrap();

        let handle =
            ObjectStore::open_object(&store, object.object_id(), HandleOptions::default(), None)
                .await
                .expect("open_object failed");

        assert!(handle.is_verified_file());

        let mut buf = object.allocate_buffer(file_size as usize).await;
        assert_eq!(
            handle.read(0, buf.as_mut()).await.expect("Read whole file."),
            file_size as usize
        );

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_verify_data_corrupt_tree() {
        let fs: OpenFxFilesystem = test_filesystem().await;
        let object_id = {
            let store = fs.root_store();
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let object = Arc::new(
                ObjectStore::create_object(
                    &store,
                    &mut transaction,
                    HandleOptions::default(),
                    None,
                )
                .await
                .expect("create_object failed"),
            );
            let object_id = object.object_id();

            transaction.commit().await.unwrap();

            let mut buf = object.allocate_buffer(5 * fs.block_size().get() as usize).await;
            buf.fill(123);
            object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

            object
                .enable_verity(fio::VerificationOptions {
                    hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                    salt: Some(vec![]),
                    ..Default::default()
                })
                .await
                .expect("set verified file metadata failed");
            object.read(0, buf.as_mut()).await.expect("verified read");

            // Corrupt the merkle tree before closing.
            let mut merkle = object
                .read_attr(AttributeId::FSVERITY_MERKLE)
                .await
                .unwrap()
                .expect("Reading merkle tree");
            merkle[0] = merkle[0].wrapping_add(1);
            object
                .write_attr(AttributeId::FSVERITY_MERKLE, &*merkle)
                .await
                .expect("Overwriting merkle");

            object_id
        }; // Close object.

        // Reopening the object should complain about the corrupted merkle tree.
        assert!(
            ObjectStore::open_object(&fs.root_store(), object_id, HandleOptions::default(), None)
                .await
                .is_err()
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_allocate_verity_file() {
        let fs: OpenFxFilesystem = test_filesystem().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        let object = Arc::new(
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed"),
        );
        transaction.commit().await.unwrap();

        let mut buf = object.allocate_buffer(8192).await;
        buf.fill(0xAA);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        object
            .enable_verity(fio::VerificationOptions {
                hash_algorithm: Some(fio::HashAlgorithm::Sha256),
                salt: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("enable_verity failed");

        assert!(object.is_verified_file());

        // Calling allocate on a verity-enabled file should return an error.
        assert!(object.allocate(0..8192).await.is_err());

        // Even after opening the object again (simulating cache eviction), it must remain verified.
        let reopened =
            ObjectStore::open_object(&store, object.object_id(), HandleOptions::default(), None)
                .await
                .expect("open_object failed");
        assert!(reopened.is_verified_file());

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_extend() {
        let fs = test_filesystem().await;
        let handle;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        handle =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");

        // As of writing, an empty filesystem has two 512kiB superblock extents and a little over
        // 256kiB of additional allocations (journal, etc) so we start use a 'magic' starting point
        // of 2MiB here.
        const START_OFFSET: u64 = 2048 * 1024;
        handle
            .extend(&mut transaction, START_OFFSET..START_OFFSET + 5 * fs.block_size())
            .await
            .expect("extend failed");
        transaction.commit().await.expect("commit failed");
        let mut buf = handle.allocate_buffer(5 * fs.block_size().get() as usize).await;
        buf.fill(123);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        buf.fill(67);
        handle.read(0, buf.as_mut()).await.expect("read failed");
        assert_eq!(buf.to_vec(), vec![123; 5 * fs.block_size().get() as usize]);
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_truncate_deallocates_old_extents() {
        let (fs, object) = test_filesystem_and_object().await;
        let mut buf = object.allocate_buffer(5 * fs.block_size().get() as usize).await;
        buf.fill(0xaa);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");

        let allocator = fs.allocator();
        let allocated_before = allocator.get_allocated_bytes();
        object.truncate(fs.block_size().get()).await.expect("truncate failed");
        let allocated_after = allocator.get_allocated_bytes();
        assert!(
            allocated_after < allocated_before,
            "before = {} after = {}",
            allocated_before,
            allocated_after
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_truncate_zeroes_tail_block() {
        let (fs, object) = test_filesystem_and_object().await;

        WriteObjectHandle::truncate(&object, TEST_DATA_OFFSET + 3).await.expect("truncate failed");
        WriteObjectHandle::truncate(&object, TEST_DATA_OFFSET + TEST_DATA.len() as u64)
            .await
            .expect("truncate failed");

        let mut buf = object.allocate_buffer(fs.block_size().get() as usize).await;
        let offset = (TEST_DATA_OFFSET % fs.block_size()) as usize;
        object.read(TEST_DATA_OFFSET - offset as u64, buf.as_mut()).await.expect("read failed");

        let mut expected = TEST_DATA.to_vec();
        expected[3..].fill(0);
        assert_eq!(
            &buf.as_ptr_slice().subslice(offset..offset + expected.len()).to_vec()[..],
            &expected
        );
    }

    #[fuchsia::test]
    async fn test_trim() {
        // Format a new filesystem.
        let device = DeviceHolder::new(FakeDevice::new(8192, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let block_size = fs.block_size();
        root_volume(fs.clone())
            .await
            .expect("root_volume failed")
            .new_volume("test", NewChildStoreOptions::default())
            .await
            .expect("volume failed");
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        // To test trim, we open the filesystem and set up a post commit hook that runs after every
        // transaction.  When the hook triggers, we can fsck the volume, take a snapshot of the
        // device and check that it gets replayed correctly on the snapshot.  We can check that the
        // graveyard trims the file as expected.
        #[derive(Default)]
        struct Context {
            store: Option<Arc<ObjectStore>>,
            object_id: Option<u64>,
        }
        let shared_context = Arc::new(Mutex::new(Context::default()));

        let object_size = (TRANSACTION_MUTATION_THRESHOLD as u64 + 10) * 2 * block_size;

        // Wait for an object to get tombstoned by the graveyard.
        async fn expect_tombstoned(store: &Arc<ObjectStore>, object_id: u64) {
            loop {
                if let Err(e) =
                    ObjectStore::open_object(store, object_id, HandleOptions::default(), None).await
                {
                    assert!(
                        FxfsError::NotFound.matches(&e),
                        "open_object didn't fail with NotFound: {:?}",
                        e
                    );
                    break;
                }
                // The graveyard should eventually tombstone the object.
                fasync::Timer::new(std::time::Duration::from_millis(100)).await;
            }
        }

        // Checks to see if the object needs to be trimmed.
        async fn needs_trim(store: &Arc<ObjectStore>) -> Option<DataObjectHandle<ObjectStore>> {
            let root_directory = Directory::open(store, store.root_directory_object_id())
                .await
                .expect("open failed");
            let oid = root_directory.lookup("foo").await.expect("lookup failed");
            if let Some((oid, _, _)) = oid {
                let object = ObjectStore::open_object(store, oid, HandleOptions::default(), None)
                    .await
                    .expect("open_object failed");
                let props = object.get_properties().await.expect("get_properties failed");
                if props.allocated_size > 0 && props.data_attribute_size == 0 {
                    Some(object)
                } else {
                    None
                }
            } else {
                None
            }
        }

        let shared_context_clone = shared_context.clone();
        let post_commit = move || {
            let store = shared_context_clone.lock().store.as_ref().cloned().unwrap();
            let shared_context = shared_context_clone.clone();
            async move {
                // First run fsck on the current filesystem.
                let options = FsckOptions {
                    fail_on_warning: true,
                    no_lock: true,
                    on_error: Box::new(|err| println!("fsck error: {:?}", err)),
                    ..Default::default()
                };
                let fs = store.filesystem();

                fsck_with_options(fs.clone(), &options).await.expect("fsck_with_options failed");
                fsck_volume_with_options(fs.as_ref(), &options, store.store_object_id(), None)
                    .await
                    .expect("fsck_volume_with_options failed");

                // Now check that we can replay this correctly.
                fs.sync(SyncOptions { flush_device: true, ..Default::default() })
                    .await
                    .expect("sync failed");
                let device = fs.device().snapshot().expect("snapshot failed");

                let object_id = shared_context.lock().object_id.clone();

                let fs2 = FxFilesystemBuilder::new()
                    .skip_initial_reap(object_id.is_none())
                    .open(device)
                    .await
                    .expect("open failed");

                // If the "foo" file exists check that allocated size matches content size.
                let root_vol = root_volume(fs2.clone()).await.expect("root_volume failed");
                let store =
                    root_vol.volume("test", StoreOptions::default()).await.expect("volume failed");

                if let Some(oid) = object_id {
                    // For the second pass, the object should get tombstoned.
                    expect_tombstoned(&store, oid).await;
                } else if let Some(object) = needs_trim(&store).await {
                    // Extend the file and make sure that it is correctly trimmed.
                    object.truncate(object_size).await.expect("truncate failed");
                    let mut buf = object.allocate_buffer(block_size.get() as usize).await;
                    object
                        .read(object_size - block_size * 2, buf.as_mut())
                        .await
                        .expect("read failed");
                    assert_eq!(buf.to_vec(), vec![0; block_size.get() as usize]);

                    // Remount, this time with the graveyard performing an initial reap and the
                    // object should get trimmed.
                    let fs = FxFilesystem::open(fs.device().snapshot().expect("snapshot failed"))
                        .await
                        .expect("open failed");
                    let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
                    let store = root_vol
                        .volume("test", StoreOptions::default())
                        .await
                        .expect("volume failed");
                    while needs_trim(&store).await.is_some() {
                        // The object has been truncated, but still has some data allocated to
                        // it.  The graveyard should trim the object eventually.
                        fasync::Timer::new(std::time::Duration::from_millis(100)).await;
                    }

                    // Run fsck.
                    fsck_with_options(fs.clone(), &options)
                        .await
                        .expect("fsck_with_options failed");
                    fsck_volume_with_options(fs.as_ref(), &options, store.store_object_id(), None)
                        .await
                        .expect("fsck_volume_with_options failed");
                    fs.close().await.expect("close failed");
                }

                // Run fsck on fs2.
                fsck_with_options(fs2.clone(), &options).await.expect("fsck_with_options failed");
                fsck_volume_with_options(fs2.as_ref(), &options, store.store_object_id(), None)
                    .await
                    .expect("fsck_volume_with_options failed");
                fs2.close().await.expect("close failed");
            }
            .boxed()
        };

        let fs = FxFilesystemBuilder::new()
            .post_commit_hook(post_commit)
            .open(device)
            .await
            .expect("open failed");

        let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
        let store = root_vol.volume("test", StoreOptions::default()).await.expect("volume failed");

        shared_context.lock().store = Some(store.clone());

        let root_directory =
            Directory::open(&store, store.root_directory_object_id()).await.expect("open failed");

        let object;
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
        object = root_directory
            .create_child_file(&mut transaction, "foo")
            .await
            .expect("create_object failed");
        transaction.commit().await.expect("commit failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), object.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");

        // Two passes: first with a regular object, and then with that object moved into the
        // graveyard.
        let mut pass = 0;
        loop {
            // Create enough extents in it such that when we truncate the object it will require
            // more than one transaction.
            let mut buf = object.allocate_buffer(5).await;
            buf.fill(1);
            // Write every other block.
            for offset in (0..object_size).into_iter().step_by((2 * block_size) as usize) {
                object
                    .txn_write(&mut transaction, offset, buf.as_ref())
                    .await
                    .expect("write failed");
            }
            transaction.commit().await.expect("commit failed");
            // This should take up more than one transaction.
            WriteObjectHandle::truncate(&object, 0).await.expect("truncate failed");

            if pass == 1 {
                break;
            }

            // Store the object ID so that we can make sure the object is always tombstoned
            // after remount (see above).
            shared_context.lock().object_id = Some(object.object_id());

            transaction = fs
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

            // Move the object into the graveyard.
            replace_child(&mut transaction, None, (&root_directory, "foo"))
                .await
                .expect("replace_child failed");
            store.add_to_graveyard(&mut transaction, object.object_id());

            pass += 1;
        }

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_adjust_refs() {
        let (fs, object) = test_filesystem_and_object().await;
        let store = object.owner();
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), object.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_eq!(
            store
                .adjust_refs(&mut transaction, object.object_id(), 1)
                .await
                .expect("adjust_refs failed"),
            false
        );
        transaction.commit().await.expect("commit failed");

        let allocator = fs.allocator();
        let allocated_before = allocator.get_allocated_bytes();
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(store.store_object_id(), object.object_id())],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        assert_eq!(
            store
                .adjust_refs(&mut transaction, object.object_id(), -2)
                .await
                .expect("adjust_refs failed"),
            true
        );
        transaction.commit().await.expect("commit failed");

        assert_eq!(allocator.get_allocated_bytes(), allocated_before);

        store
            .tombstone_object(
                object.object_id(),
                Options { borrow_metadata_space: true, ..Default::default() },
                None,
            )
            .await
            .expect("purge failed");

        assert_eq!(allocated_before - allocator.get_allocated_bytes(), fs.block_size());

        // We need to remove the directory entry, too, otherwise fsck will complain
        {
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
            let root_directory = Directory::open(&store, store.root_directory_object_id())
                .await
                .expect("open failed");
            transaction.add(
                store.store_object_id(),
                Mutation::replace_or_insert_object(
                    ObjectKey::child(root_directory.object_id(), TEST_OBJECT_NAME, DirType::Normal),
                    ObjectValue::None,
                ),
            );
            transaction.commit().await.expect("commit failed");
        }

        fsck_with_options(
            fs.clone(),
            &FsckOptions {
                fail_on_warning: true,
                on_error: Box::new(|err| println!("fsck error: {:?}", err)),
                ..Default::default()
            },
        )
        .await
        .expect("fsck_with_options failed");

        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_locks() {
        let (fs, object) = test_filesystem_and_object().await;
        let (send1, recv1) = channel();
        let (send2, recv2) = channel();
        let (send3, recv3) = channel();
        let done = Mutex::new(false);
        let mut futures = FuturesUnordered::new();
        futures.push(
            async {
                let mut t = object.new_transaction().await.expect("new_transaction failed");
                send1.send(()).unwrap(); // Tell the next future to continue.
                send3.send(()).unwrap(); // Tell the last future to continue.
                recv2.await.unwrap();
                let mut buf = object.allocate_buffer(5).await;
                buf.copy_from_slice(b"hello");
                object.txn_write(&mut t, 0, buf.as_ref()).await.expect("write failed");
                // This is a halting problem so all we can do is sleep.
                fasync::Timer::new(Duration::from_millis(100)).await;
                assert!(!*done.lock());
                t.commit().await.expect("commit failed");
            }
            .boxed(),
        );
        futures.push(
            async {
                recv1.await.unwrap();
                // Reads should not block.
                let offset = TEST_DATA_OFFSET as usize;
                let align = (offset as u64 % fs.block_size()) as usize;
                let len = TEST_DATA.len();
                let mut buf = object.allocate_buffer(align + len).await;
                assert_eq!(
                    object.read((offset - align) as u64, buf.as_mut()).await.expect("read failed"),
                    align + TEST_DATA.len()
                );
                assert_eq!(&buf.as_ptr_slice().subslice(align..buf.len()).to_vec()[..], TEST_DATA);
                // Tell the first future to continue.
                send2.send(()).unwrap();
            }
            .boxed(),
        );
        futures.push(
            async {
                // This should block until the first future has completed.
                recv3.await.unwrap();
                let _t = object.new_transaction().await.expect("new_transaction failed");
                let mut buf = object.allocate_buffer(5).await;
                assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed"), 5);
                assert_eq!(buf.to_vec(), b"hello");
            }
            .boxed(),
        );
        while let Some(()) = futures.next().await {}
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_racy_reads() {
        let fs = test_filesystem().await;
        let object;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let store = fs.root_store();
        object = Arc::new(
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed"),
        );
        transaction.commit().await.expect("commit failed");
        for _ in 0..100 {
            let cloned_object = object.clone();
            let writer = fasync::Task::spawn(async move {
                let mut buf = cloned_object.allocate_buffer(10).await;
                buf.fill(123);
                cloned_object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
            });
            let cloned_object = object.clone();
            let reader = fasync::Task::spawn(async move {
                let wait_time = rand::random_range(0..5);
                fasync::Timer::new(Duration::from_millis(wait_time)).await;
                let mut buf = cloned_object.allocate_buffer(10).await;
                buf.fill(23);
                let amount = cloned_object.read(0, buf.as_mut()).await.expect("write failed");
                // If we succeed in reading data, it must include the write; i.e. if we see the size
                // change, we should see the data too.  For this to succeed it requires locking on
                // the read size to ensure that when we read the size, we get the extents changed in
                // that same transaction.
                if amount != 0 {
                    assert_eq!(amount, 10);
                    assert_eq!(buf.to_vec(), &[123; 10]);
                }
            });
            writer.await;
            reader.await;
            object.truncate(0).await.expect("truncate failed");
        }
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_allocated_size() {
        let (fs, object) = test_filesystem_and_object_with_key(None, true).await;

        let before = object.get_properties().await.expect("get_properties failed").allocated_size;
        let mut buf = object.allocate_buffer(5).await;
        buf.copy_from_slice(b"hello");
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        let after = object.get_properties().await.expect("get_properties failed").allocated_size;
        assert_eq!(after, before + fs.block_size());

        // Do the same write again and there should be no change.
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        assert_eq!(
            object.get_properties().await.expect("get_properties failed").allocated_size,
            after
        );

        // extend...
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        let offset = 1000 * fs.block_size();
        let before = after;
        object
            .extend(&mut transaction, offset..offset + fs.block_size())
            .await
            .expect("extend failed");
        transaction.commit().await.expect("commit failed");
        let after = object.get_properties().await.expect("get_properties failed").allocated_size;
        assert_eq!(after, before + fs.block_size());

        // truncate...
        let before = after;
        let size = object.get_size();
        object.truncate(size - fs.block_size()).await.expect("extend failed");
        let after = object.get_properties().await.expect("get_properties failed").allocated_size;
        assert_eq!(after, before - fs.block_size());

        // preallocate_range...
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        let before = after;
        let mut file_range = offset..offset + fs.block_size();
        object.preallocate_range(&mut transaction, &mut file_range).await.expect("extend failed");
        transaction.commit().await.expect("commit failed");
        let after = object.get_properties().await.expect("get_properties failed").allocated_size;
        assert_eq!(after, before + fs.block_size());
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_zero() {
        let (fs, object) = test_filesystem_and_object().await;
        let expected_size = object.get_size();
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        object.zero(&mut transaction, 0..fs.block_size() * 10).await.expect("zero failed");
        transaction.commit().await.expect("commit failed");
        assert_eq!(object.get_size(), expected_size);
        let mut buf = object.allocate_buffer((fs.block_size() * 10) as usize).await;
        assert_eq!(object.read(0, buf.as_mut()).await.expect("read failed") as u64, expected_size);
        assert_eq!(
            &buf.as_ptr_slice().subslice(0..expected_size as usize).to_vec()[..],
            vec![0u8; expected_size as usize].as_slice()
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_properties() {
        let (fs, object) = test_filesystem_and_object().await;
        const CRTIME: Timestamp = Timestamp::from_nanos(1234);
        const MTIME: Timestamp = Timestamp::from_nanos(5678);
        const CTIME: Timestamp = Timestamp::from_nanos(8765);

        // ObjectProperties can be updated through `update_attributes`.
        // `get_properties` should reflect the latest changes.
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        object
            .update_attributes(
                &mut transaction,
                Some(&fio::MutableNodeAttributes {
                    creation_time: Some(CRTIME.as_nanos()),
                    modification_time: Some(MTIME.as_nanos()),
                    mode: Some(111),
                    gid: Some(222),
                    ..Default::default()
                }),
                None,
            )
            .await
            .expect("update_attributes failed");
        const MTIME_NEW: Timestamp = Timestamp::from_nanos(12345678);
        object
            .update_attributes(
                &mut transaction,
                Some(&fio::MutableNodeAttributes {
                    modification_time: Some(MTIME_NEW.as_nanos()),
                    gid: Some(333),
                    rdev: Some(444),
                    ..Default::default()
                }),
                Some(CTIME),
            )
            .await
            .expect("update_timestamps failed");
        transaction.commit().await.expect("commit failed");

        let properties = object.get_properties().await.expect("get_properties failed");
        assert_matches!(
            properties,
            ObjectProperties {
                refs: 1u64,
                allocated_size: TEST_OBJECT_ALLOCATED_SIZE,
                data_attribute_size: TEST_OBJECT_SIZE,
                creation_time: CRTIME,
                modification_time: MTIME_NEW,
                posix_attributes: Some(PosixAttributes { mode: 111, gid: 333, rdev: 444, .. }),
                change_time: CTIME,
                ..
            }
        );
        fs.close().await.expect("Close failed");
    }

    #[fuchsia::test]
    async fn test_is_allocated() {
        let (fs, object) = test_filesystem_and_object().await;

        // `test_filesystem_and_object()` wrote the buffer `TEST_DATA` to the device at offset
        // `TEST_DATA_OFFSET` where the length and offset are aligned to the block size.
        let aligned_offset = fs.block_size().align_down(TEST_DATA_OFFSET);
        let aligned_length = fs.block_size().align_up(TEST_DATA.len() as u64).unwrap();

        // Check for the case where where we have the following extent layout
        //       [ unallocated ][ `TEST_DATA` ]
        // The extents before `aligned_offset` should not be allocated
        let (allocated, count) = object.is_allocated(0).await.expect("is_allocated failed");
        assert_eq!(count, aligned_offset);
        assert_eq!(allocated, false);

        let (allocated, count) =
            object.is_allocated(aligned_offset).await.expect("is_allocated failed");
        assert_eq!(count, aligned_length);
        assert_eq!(allocated, true);

        // Check for the case where where we query out of range
        let end = aligned_offset + aligned_length;
        object
            .is_allocated(end)
            .await
            .expect_err("is_allocated should have returned ERR_OUT_OF_RANGE");

        // Check for the case where where we start querying for allocation starting from
        // an allocated range to the end of the device
        let size = 50 * fs.block_size();
        object.truncate(size).await.expect("extend failed");

        let (allocated, count) = object.is_allocated(end).await.expect("is_allocated failed");
        assert_eq!(count, size - end);
        assert_eq!(allocated, false);

        // Check for the case where where we have the following extent layout
        //      [ unallocated ][ `buf` ][ `buf` ]
        let buf_length = 5 * fs.block_size();
        let mut buf = object.allocate_buffer(buf_length as usize).await;
        buf.fill(123);
        let new_offset = end + 20 * fs.block_size();
        object.write_or_append(Some(new_offset), buf.as_ref()).await.expect("write failed");
        object
            .write_or_append(Some(new_offset + buf_length), buf.as_ref())
            .await
            .expect("write failed");

        let (allocated, count) = object.is_allocated(end).await.expect("is_allocated failed");
        assert_eq!(count, new_offset - end);
        assert_eq!(allocated, false);

        let (allocated, count) =
            object.is_allocated(new_offset).await.expect("is_allocated failed");
        assert_eq!(count, 2 * buf_length);
        assert_eq!(allocated, true);

        // Check the case where we query from the middle of an extent
        let (allocated, count) = object
            .is_allocated(new_offset + 4 * fs.block_size())
            .await
            .expect("is_allocated failed");
        assert_eq!(count, 2 * buf_length - 4 * fs.block_size());
        assert_eq!(allocated, true);

        // Now, write buffer to a location already written to.
        // Check for the case when we the following extent layout
        //      [ unallocated ][ `other_buf` ][ (part of) `buf` ][ `buf` ]
        let other_buf_length = 3 * fs.block_size();
        let mut other_buf = object.allocate_buffer(other_buf_length as usize).await;
        other_buf.fill(231);
        object.write_or_append(Some(new_offset), other_buf.as_ref()).await.expect("write failed");

        // We still expect that `is_allocated(..)` will return that  there are 2*`buf_length bytes`
        // allocated from `new_offset`
        let (allocated, count) =
            object.is_allocated(new_offset).await.expect("is_allocated failed");
        assert_eq!(count, 2 * buf_length);
        assert_eq!(allocated, true);

        // Check for the case when we the following extent layout
        //   [ unallocated ][ deleted ][ unallocated ][ deleted ][ allocated ]
        // Mark TEST_DATA as deleted
        let mut transaction = object.new_transaction().await.expect("new_transaction failed");
        object
            .zero(&mut transaction, aligned_offset..aligned_offset + aligned_length)
            .await
            .expect("zero failed");
        // Mark `other_buf` as deleted
        object
            .zero(&mut transaction, new_offset..new_offset + buf_length)
            .await
            .expect("zero failed");
        transaction.commit().await.expect("commit transaction failed");

        let (allocated, count) = object.is_allocated(0).await.expect("is_allocated failed");
        assert_eq!(count, new_offset + buf_length);
        assert_eq!(allocated, false);

        let (allocated, count) =
            object.is_allocated(new_offset + buf_length).await.expect("is_allocated failed");
        assert_eq!(count, buf_length);
        assert_eq!(allocated, true);

        let new_end = new_offset + buf_length + count;

        // Check for the case where there are objects with different keys.
        // Case that we're checking for:
        //      [ unallocated ][ extent (object with different key) ][ unallocated ]
        let store = object.owner();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let object2 =
            ObjectStore::create_object(&store, &mut transaction, HandleOptions::default(), None)
                .await
                .expect("create_object failed");
        transaction.commit().await.expect("commit failed");

        object2
            .write_or_append(Some(new_end + fs.block_size()), buf.as_ref())
            .await
            .expect("write failed");

        // Expecting that the extent with a different key is treated like unallocated extent
        let (allocated, count) = object.is_allocated(new_end).await.expect("is_allocated failed");
        assert_eq!(count, size - new_end);
        assert_eq!(allocated, false);

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_read_write_attr() {
        let (_fs, object) = test_filesystem_and_object().await;
        let data = [0xffu8; 16_384];
        object.write_attr(AttributeId(20), &data).await.expect("write_attr failed");
        let rdata = object
            .read_attr(AttributeId(20))
            .await
            .expect("read_attr failed")
            .expect("no attribute data found");
        assert_eq!(&data[..], &rdata[..]);

        assert_eq!(object.read_attr(AttributeId(21)).await.expect("read_attr failed"), None);
    }

    #[fuchsia::test(threads = 10)]
    async fn test_allocate_basic() {
        let (fs, object) = test_filesystem_and_empty_object().await;
        let block_size = fs.block_size();
        let file_size = block_size * 10;
        object.truncate(file_size).await.unwrap();

        let small_buf_size = 1024;
        let large_buf_aligned_size = (block_size * 2) as usize;
        let large_buf_size = (block_size * 2 + 1024) as usize;

        let mut small_buf = object.allocate_buffer(small_buf_size).await;
        let mut large_buf_aligned = object.allocate_buffer(large_buf_aligned_size).await;
        let mut large_buf = object.allocate_buffer(large_buf_size).await;

        assert_eq!(object.read(0, small_buf.as_mut()).await.unwrap(), small_buf_size);
        assert_eq!(small_buf.to_vec(), vec![0; small_buf_size]);
        assert_eq!(object.read(0, large_buf.as_mut()).await.unwrap(), large_buf_size);
        assert_eq!(large_buf.to_vec(), vec![0; large_buf_size]);
        assert_eq!(
            object.read(0, large_buf_aligned.as_mut()).await.unwrap(),
            large_buf_aligned_size
        );
        assert_eq!(large_buf_aligned.to_vec(), vec![0; large_buf_aligned_size]);

        // Allocation succeeds, and without any writes to the location it shows up as zero.
        object.allocate(block_size.get()..block_size * 3).await.unwrap();

        // Test starting before, inside, and after the allocated section with every sized buffer.
        for (buf_index, buf) in [small_buf, large_buf, large_buf_aligned].iter_mut().enumerate() {
            for offset in 0..4 {
                assert_eq!(
                    object.read(block_size * offset, buf.as_mut()).await.unwrap(),
                    buf.len(),
                    "buf_index: {}, read offset: {}",
                    buf_index,
                    offset,
                );
                assert_eq!(
                    &buf.to_vec(),
                    &vec![0; buf.len()],
                    "buf_index: {}, read offset: {}",
                    buf_index,
                    offset,
                );
            }
        }

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_allocate_extends_file() {
        const BUF_SIZE: usize = 1024;
        let (fs, object) = test_filesystem_and_empty_object().await;
        let mut buf = object.allocate_buffer(BUF_SIZE).await;
        let block_size = fs.block_size();

        assert_eq!(object.read(0, buf.as_mut()).await.unwrap(), buf.len());
        assert_eq!(buf.to_vec(), &[0; BUF_SIZE]);

        assert!(TEST_OBJECT_SIZE < block_size * 4);
        // Allocation succeeds, and without any writes to the location it shows up as zero.
        object.allocate(0..block_size * 4).await.unwrap();
        assert_eq!(object.read(0, buf.as_mut()).await.unwrap(), buf.len());
        assert_eq!(buf.to_vec(), &[0; BUF_SIZE]);
        assert_eq!(object.read(block_size.get(), buf.as_mut()).await.unwrap(), buf.len());
        assert_eq!(buf.to_vec(), &[0; BUF_SIZE]);
        assert_eq!(object.read(block_size * 3, buf.as_mut()).await.unwrap(), buf.len());
        assert_eq!(buf.to_vec(), &[0; BUF_SIZE]);

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_allocate_past_end() {
        const BUF_SIZE: usize = 1024;
        let (fs, object) = test_filesystem_and_empty_object().await;
        let mut buf = object.allocate_buffer(BUF_SIZE).await;
        let block_size = fs.block_size();

        assert_eq!(object.read(0, buf.as_mut()).await.unwrap(), buf.len());
        assert_eq!(buf.to_vec(), &[0; BUF_SIZE]);

        assert!(TEST_OBJECT_SIZE < block_size * 4);
        // Allocation succeeds, and without any writes to the location it shows up as zero.
        object.allocate(block_size * 4..block_size * 6).await.unwrap();
        assert_eq!(object.read(0, buf.as_mut()).await.unwrap(), buf.len());
        assert_eq!(buf.to_vec(), &[0; BUF_SIZE]);
        assert_eq!(object.read(block_size * 4, buf.as_mut()).await.unwrap(), buf.len());
        assert_eq!(buf.to_vec(), &[0; BUF_SIZE]);
        assert_eq!(object.read(block_size * 5, buf.as_mut()).await.unwrap(), buf.len());
        assert_eq!(buf.to_vec(), &[0; BUF_SIZE]);

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_allocate_read_attr() {
        let (fs, object) = test_filesystem_and_empty_object().await;
        let block_size = fs.block_size();
        let file_size = block_size * 4;
        object.truncate(file_size).await.unwrap();

        let content = object
            .read_attr(object.attribute_id())
            .await
            .expect("failed to read attr")
            .expect("attr returned none");
        assert_eq!(content.as_ref(), &vec![0; file_size as usize]);

        object.allocate(block_size.get()..block_size * 3).await.unwrap();

        let content = object
            .read_attr(object.attribute_id())
            .await
            .expect("failed to read attr")
            .expect("attr returned none");
        assert_eq!(content.as_ref(), &vec![0; file_size as usize]);

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_allocate_existing_data() {
        struct Case {
            written_ranges: Vec<Range<usize>>,
            allocate_range: Range<u64>,
        }
        let cases = [
            Case { written_ranges: vec![4..7], allocate_range: 4..7 },
            Case { written_ranges: vec![4..7], allocate_range: 3..8 },
            Case { written_ranges: vec![4..7], allocate_range: 5..6 },
            Case { written_ranges: vec![4..7], allocate_range: 5..8 },
            Case { written_ranges: vec![4..7], allocate_range: 3..5 },
            Case { written_ranges: vec![0..1, 2..3, 4..5, 6..7, 8..9], allocate_range: 0..10 },
            Case { written_ranges: vec![0..2, 4..6, 7..10], allocate_range: 1..8 },
        ];

        for case in cases {
            let (fs, object) = test_filesystem_and_empty_object().await;
            let block_size = fs.block_size();
            let file_size = block_size * 10;
            object.truncate(file_size).await.unwrap();

            for write in &case.written_ranges {
                let write_len = (write.end - write.start) * block_size.get() as usize;
                let mut write_buf = object.allocate_buffer(write_len).await;
                write_buf.fill(0xff);
                assert_eq!(
                    object
                        .write_or_append(Some(block_size * write.start as u64), write_buf.as_ref())
                        .await
                        .unwrap(),
                    file_size
                );
            }

            let mut expected_buf = object.allocate_buffer(file_size as usize).await;
            assert_eq!(object.read(0, expected_buf.as_mut()).await.unwrap(), expected_buf.len());

            object
                .allocate(
                    case.allocate_range.start * block_size..case.allocate_range.end * block_size,
                )
                .await
                .unwrap();

            let mut read_buf = object.allocate_buffer(file_size as usize).await;
            assert_eq!(object.read(0, read_buf.as_mut()).await.unwrap(), read_buf.len());
            assert_eq!(read_buf.to_vec(), expected_buf.to_vec());

            fs.close().await.expect("close failed");
        }
    }

    async fn get_modes(
        obj: &DataObjectHandle<ObjectStore>,
        mut search_range: Range<u64>,
    ) -> Vec<(Range<u64>, ExtentMode)> {
        let mut modes = Vec::new();
        let store = obj.store();
        let tree = store.tree();
        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::attribute(
                obj.object_id(),
                AttributeId::DATA,
                AttributeKey::Extent(Extent::search_key_from_offset(search_range.start)),
            )))
            .await
            .unwrap();
        loop {
            match iter.get() {
                Some(ItemRef {
                    key:
                        ObjectKey {
                            object_id,
                            data:
                                ObjectKeyData::Attribute(
                                    AttributeId::DATA,
                                    AttributeKey::Extent(extent),
                                ),
                        },
                    value: ObjectValue::Extent(ExtentValue::Some { mode, .. }),
                    ..
                }) if *object_id == obj.object_id() => {
                    if search_range.end <= extent.start {
                        break;
                    }
                    let found_range = std::cmp::max(search_range.start, extent.start)
                        ..std::cmp::min(search_range.end, extent.end);
                    search_range.start = found_range.end;
                    modes.push((found_range, mode.clone()));
                    if search_range.start == search_range.end {
                        break;
                    }
                    iter.advance().await.unwrap();
                }
                x => panic!("looking for extent record, found this {:?}", x),
            }
        }
        modes
    }

    async fn assert_all_overwrite(
        obj: &DataObjectHandle<ObjectStore>,
        mut search_range: Range<u64>,
    ) {
        let modes = get_modes(obj, search_range.clone()).await;
        for mode in modes {
            assert_eq!(
                mode.0.start, search_range.start,
                "missing mode in range {}..{}",
                search_range.start, mode.0.start
            );
            match mode.1 {
                ExtentMode::Overwrite | ExtentMode::OverwritePartial(_) => (),
                m => panic!("mode at range {:?} was not overwrite, instead found {:?}", mode.0, m),
            }
            assert!(
                mode.0.end <= search_range.end,
                "mode ends beyond search range (bug in test) - search_range: {:?}, mode: {:?}",
                search_range,
                mode,
            );
            search_range.start = mode.0.end;
        }
        assert_eq!(
            search_range.start, search_range.end,
            "missing mode in range {:?}",
            search_range
        );
    }

    #[fuchsia::test(threads = 10)]
    async fn test_multi_overwrite() {
        #[derive(Debug)]
        struct Case {
            pre_writes: Vec<Range<usize>>,
            allocate_ranges: Vec<Range<u64>>,
            overwrites: Vec<Vec<Range<u64>>>,
        }
        let cases = [
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![1..3],
                overwrites: vec![vec![1..3]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..1, 1..2, 2..3, 3..4],
                overwrites: vec![vec![0..4]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..4],
                overwrites: vec![vec![0..1], vec![1..2], vec![3..4]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..4],
                overwrites: vec![vec![3..4]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..4],
                overwrites: vec![vec![3..4], vec![2..3], vec![1..2]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![1..2, 5..6, 7..8],
                overwrites: vec![vec![5..6]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![1..3],
                overwrites: vec![
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                ],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..5],
                overwrites: vec![
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                    vec![1..3],
                ],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..5],
                overwrites: vec![vec![0..2, 2..4, 4..5]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..5, 5..10],
                overwrites: vec![vec![1..2, 2..3, 4..7, 7..8]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..4, 6..10],
                overwrites: vec![vec![2..3, 7..9]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..10],
                overwrites: vec![vec![1..2, 5..10], vec![0..1, 5..10], vec![0..5, 5..10]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..10],
                overwrites: vec![vec![0..2, 2..4, 4..6, 6..8, 8..10], vec![0..5, 5..10]],
            },
            Case {
                pre_writes: vec![1..3],
                allocate_ranges: vec![1..3],
                overwrites: vec![vec![1..3]],
            },
            Case {
                pre_writes: vec![1..3],
                allocate_ranges: vec![4..6],
                overwrites: vec![vec![5..6]],
            },
            Case {
                pre_writes: vec![1..3],
                allocate_ranges: vec![0..4],
                overwrites: vec![vec![0..4]],
            },
            Case {
                pre_writes: vec![1..3],
                allocate_ranges: vec![2..4],
                overwrites: vec![vec![2..4]],
            },
            Case {
                pre_writes: vec![3..5],
                allocate_ranges: vec![1..3, 6..7],
                overwrites: vec![vec![1..3, 6..7]],
            },
            Case {
                pre_writes: vec![1..3, 5..7, 8..9],
                allocate_ranges: vec![0..5],
                overwrites: vec![vec![0..2, 2..5], vec![0..5]],
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![0..10, 4..6],
                overwrites: Vec::new(),
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![3..8, 5..10],
                overwrites: Vec::new(),
            },
            Case {
                pre_writes: Vec::new(),
                allocate_ranges: vec![5..10, 3..8],
                overwrites: Vec::new(),
            },
        ];

        for (i, case) in cases.into_iter().enumerate() {
            log::info!("running case {} - {:?}", i, case);
            let (fs, object) = test_filesystem_and_empty_object().await;
            let block_size = fs.block_size();
            let file_size = block_size * 10;
            object.truncate(file_size).await.unwrap();

            for write in case.pre_writes {
                let write_len = (write.end - write.start) * block_size.get() as usize;
                let mut write_buf = object.allocate_buffer(write_len).await;
                write_buf.fill(0xff);
                assert_eq!(
                    object
                        .write_or_append(Some(block_size * write.start as u64), write_buf.as_ref())
                        .await
                        .unwrap(),
                    file_size
                );
            }

            for allocate_range in &case.allocate_ranges {
                object
                    .allocate(allocate_range.start * block_size..allocate_range.end * block_size)
                    .await
                    .unwrap();
            }

            for allocate_range in case.allocate_ranges {
                assert_all_overwrite(
                    &object,
                    allocate_range.start * block_size..allocate_range.end * block_size,
                )
                .await;
            }

            for overwrite in case.overwrites {
                let mut write_len = 0;
                let overwrite = overwrite
                    .into_iter()
                    .map(|r| {
                        write_len += (r.end - r.start) * block_size;
                        r.start * block_size..r.end * block_size
                    })
                    .collect::<Vec<_>>();
                let mut write_buf = object.allocate_buffer(write_len as usize).await;
                let data = (0..20).cycle().take(write_len as usize).collect::<Vec<_>>();
                write_buf.copy_from_slice(&data);

                let mut expected_buf = object.allocate_buffer(file_size as usize).await;
                assert_eq!(
                    object.read(0, expected_buf.as_mut()).await.unwrap(),
                    expected_buf.len()
                );
                let mut expected_buf_slice = expected_buf.as_mut_ptr_slice();
                let mut data_slice = data.as_slice();
                for r in &overwrite {
                    let len = r.length().unwrap() as usize;
                    let (copy_from, rest) = data_slice.split_at(len);
                    expected_buf_slice
                        .subslice_mut(r.start as usize..r.end as usize)
                        .copy_from_slice(&copy_from);
                    data_slice = rest;
                }

                let mut transaction = object.new_transaction().await.unwrap();
                object
                    .multi_overwrite(
                        &mut transaction,
                        AttributeId::DATA,
                        &overwrite,
                        write_buf.as_mut(),
                    )
                    .await
                    .unwrap_or_else(|_| panic!("multi_overwrite error on case {}", i));
                // Double check the emitted checksums. We should have one u64 checksum for every
                // block we wrote to disk.
                let mut checksummed_range_length = 0;
                let mut num_checksums = 0;
                for (device_range, checksums, _) in transaction.checksums() {
                    let range_len = device_range.end - device_range.start;
                    let checksums_len = checksums.len() as u64;
                    assert_eq!(range_len / checksums_len, block_size);
                    checksummed_range_length += range_len;
                    num_checksums += checksums_len;
                }
                assert_eq!(checksummed_range_length, write_len);
                assert_eq!(num_checksums, write_len / block_size);
                transaction.commit().await.unwrap();

                let mut buf = object.allocate_buffer(file_size as usize).await;
                assert_eq!(
                    object.read(0, buf.as_mut()).await.unwrap(),
                    buf.len(),
                    "failed length check on case {}",
                    i,
                );
                assert_eq!(buf.to_vec(), expected_buf.to_vec(), "failed on case {}", i);
            }

            fsck_volume(&fs, object.store().store_object_id(), None).await.expect("fsck failed");
            fs.close().await.expect("close failed");
        }
    }

    #[fuchsia::test(threads = 10)]
    async fn test_multi_overwrite_mode_updates() {
        let (fs, object) = test_filesystem_and_empty_object().await;
        let block_size = fs.block_size();
        let file_size = block_size * 10;
        object.truncate(file_size).await.unwrap();

        let mut expected_bitmap = BitVec::from_elem(10, false);

        object.allocate(0..10 * block_size).await.unwrap();
        assert_eq!(
            get_modes(&object, 0..10 * block_size).await,
            vec![(0..10 * block_size, ExtentMode::OverwritePartial(expected_bitmap.clone()))]
        );

        let mut write_buf = object.allocate_buffer((2 * block_size) as usize).await;
        let data = (0..20).cycle().take(write_buf.len()).collect::<Vec<_>>();
        write_buf.copy_from_slice(&data);
        let mut transaction = object.new_transaction().await.unwrap();
        object
            .multi_overwrite(
                &mut transaction,
                AttributeId::DATA,
                &[2 * block_size..4 * block_size],
                write_buf.as_mut(),
            )
            .await
            .unwrap();
        transaction.commit().await.unwrap();

        expected_bitmap.set(2, true);
        expected_bitmap.set(3, true);
        assert_eq!(
            get_modes(&object, 0..10 * block_size).await,
            vec![(0..10 * block_size, ExtentMode::OverwritePartial(expected_bitmap.clone()))]
        );

        let mut write_buf = object.allocate_buffer((3 * block_size) as usize).await;
        let data = (0..20).cycle().take(write_buf.len()).collect::<Vec<_>>();
        write_buf.copy_from_slice(&data);
        let mut transaction = object.new_transaction().await.unwrap();
        object
            .multi_overwrite(
                &mut transaction,
                AttributeId::DATA,
                &[3 * block_size..5 * block_size, 6 * block_size..7 * block_size],
                write_buf.as_mut(),
            )
            .await
            .unwrap();
        transaction.commit().await.unwrap();

        expected_bitmap.set(4, true);
        expected_bitmap.set(6, true);
        assert_eq!(
            get_modes(&object, 0..10 * block_size).await,
            vec![(0..10 * block_size, ExtentMode::OverwritePartial(expected_bitmap.clone()))]
        );

        let mut write_buf = object.allocate_buffer((6 * block_size) as usize).await;
        let data = (0..20).cycle().take(write_buf.len()).collect::<Vec<_>>();
        write_buf.copy_from_slice(&data);
        let mut transaction = object.new_transaction().await.unwrap();
        object
            .multi_overwrite(
                &mut transaction,
                AttributeId::DATA,
                &[
                    0..2 * block_size,
                    5 * block_size..6 * block_size,
                    7 * block_size..10 * block_size,
                ],
                write_buf.as_mut(),
            )
            .await
            .unwrap();
        transaction.commit().await.unwrap();

        assert_eq!(
            get_modes(&object, 0..10 * block_size).await,
            vec![(0..10 * block_size, ExtentMode::Overwrite)]
        );

        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_check_unwritten_zero() {
        let device = DeviceHolder::new(FakeDevice::new(256 * 1024, TEST_DEVICE_BLOCK_SIZE));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let object = create_object_with_key(fs.clone(), Some(&new_insecure_crypt()), false).await;
        let block_size = fs.block_size();

        // Set up a file with eight blocks to look like this:
        // | None | COW | COW | None | Overwrite(unwritten) | Overwrite(written) | None |
        let file_size = block_size * 7;
        object.truncate(file_size).await.unwrap();
        assert!(object.check_unwritten_zero(0..file_size).await.unwrap());

        let mut buffer = object.allocate_buffer(block_size.get() as usize).await;
        buffer.fill(1);
        object
            .write_or_append(Some(block_size.get()), buffer.as_ref())
            .await
            .expect("write failed");
        object.write_or_append(Some(block_size * 2), buffer.as_ref()).await.expect("write failed");

        object.allocate((block_size * 4)..(block_size * 6)).await.expect("Allocate failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(object.store().store_object_id(), object.object_id(),)],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        object
            .multi_overwrite(
                &mut transaction,
                AttributeId::DATA,
                &vec![(block_size * 5)..(block_size * 6)],
                buffer.as_mut(),
            )
            .await
            .expect("Multi overwrite");
        transaction.commit().await.expect("Committing overwrite");

        // Anything touching the COW ranges should fail.
        assert!(!object.check_unwritten_zero(0..(block_size * 2)).await.unwrap());
        assert!(!object.check_unwritten_zero(block_size.get()..(block_size * 3)).await.unwrap());
        assert!(!object.check_unwritten_zero((block_size * 2)..(block_size * 4)).await.unwrap());

        // This should be fine, as the OverwritePartial should only touch the unwritten block.
        assert!(object.check_unwritten_zero((block_size * 3)..(block_size * 5)).await.unwrap());

        // These should touch the written overwrite block and fail.
        assert!(!object.check_unwritten_zero((block_size * 4)..(block_size * 6)).await.unwrap());
        assert!(!object.check_unwritten_zero((block_size * 5)..(block_size * 7)).await.unwrap());

        fs.close().await.expect("close failed");
    }
}
