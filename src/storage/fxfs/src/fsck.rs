// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filesystem::FxFilesystem;
use crate::fsck::errors::{FsckError, FsckFatal, FsckIssue, FsckWarning};
use crate::log::*;
use crate::lsm_tree::Query;
use crate::lsm_tree::skip_list_layer::SkipListLayer;
use crate::lsm_tree::types::{
    BoxedLayerIterator, Item, Key, Layer, LayerIterator, LayerKey, MaybeContainsKey, Value,
};
use crate::object_handle::INVALID_OBJECT_ID;
use crate::object_store::allocator::{AllocatorKey, AllocatorValue, CoalescingIterator};
use crate::object_store::journal::super_block::SuperBlockInstance;
use crate::object_store::volume::root_volume;
use crate::object_store::{ObjectStore, load_store_info};
use anyhow::{Context, Error, anyhow};
use futures::try_join;
use fxfs_crypto::Crypt;
use rustc_hash::FxHashSet as HashSet;
use std::collections::BTreeMap;
use std::iter::zip;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub mod errors;

mod store_scanner;

#[cfg(test)]
mod tests;

/// General stats about filesystem fragmentation
pub const NUM_FRAGMENTATION_HISTOGRAM_SLOTS: usize = 12;
#[derive(Default, Debug)]
pub struct FragmentationStats {
    /// Histogram of extent size in bytes. Buckets are fixed as <=4kB, <=8kB, ... <=2MiB, >2MiB.
    pub extent_size: [u64; NUM_FRAGMENTATION_HISTOGRAM_SLOTS],
    /// Histogram of extents per file. Buckets are fixed as <=1, <=2, ... <=512, >512.
    pub extent_count: [u64; NUM_FRAGMENTATION_HISTOGRAM_SLOTS],
    /// Histogram of free space in bytes. Buckets are fixed as <=4kB, <=8kB, ... <=2MiB, >2MiB.
    pub free_space: [u64; NUM_FRAGMENTATION_HISTOGRAM_SLOTS],
}

impl FragmentationStats {
    /// Returns the histogram bucket for extent_size and free_space given size in bytes.
    pub fn get_histogram_bucket_for_size(size: u64) -> usize {
        return Self::get_histogram_bucket_for_count(size / 4096);
    }
    /// Returns the histogram bucket for extent_count.
    pub fn get_histogram_bucket_for_count(count: u64) -> usize {
        let log_count = (64 - count.leading_zeros()) as usize;
        return log_count.clamp(1, NUM_FRAGMENTATION_HISTOGRAM_SLOTS) - 1;
    }
}

/// Filesystem statistics gathered on during an fsck run.
#[derive(Default, Debug)]
pub struct FsckResult {
    pub fragmentation: FragmentationStats,
}

pub struct FsckOptions<'a> {
    /// Whether to fail fsck if any warnings are encountered.
    pub fail_on_warning: bool,
    // Whether to halt after the first error encountered (fatal or not).
    pub halt_on_error: bool,
    /// Whether to perform slower, more complete checks.
    pub do_slow_passes: bool,
    /// A callback to be invoked for each detected error, e.g. to log the error.
    pub on_error: Box<dyn Fn(&FsckIssue) + Send + Sync + 'a>,
    /// If true, suppress informational messages.
    pub quiet: bool,
    /// Whether to be noisy as we do checks.
    pub verbose: bool,
    /// Don't take the write lock. The caller needs to guarantee the filesystem isn't changing.
    pub no_lock: bool,
}

impl Default for FsckOptions<'_> {
    fn default() -> Self {
        Self {
            fail_on_warning: false,
            halt_on_error: false,
            do_slow_passes: true,
            on_error: Box::new(FsckIssue::log),
            quiet: false,
            verbose: false,
            no_lock: false,
        }
    }
}

/// Verifies the integrity of Fxfs.  See errors.rs for a list of checks performed.
// TODO(https://fxbug.dev/42168496): add checks for:
//  + The root parent object store ID and root object store ID must not conflict with any other
//    stores or the allocator.
//
// TODO(https://fxbug.dev/42178152): This currently takes a write lock on the filesystem.  It would
// be nice if we could take a snapshot.
pub async fn fsck(filesystem: Arc<FxFilesystem>) -> Result<FsckResult, Error> {
    fsck_with_options(filesystem, &FsckOptions::default()).await
}

pub async fn fsck_with_options(
    filesystem: Arc<FxFilesystem>,
    options: &FsckOptions<'_>,
) -> Result<FsckResult, Error> {
    let mut result = FsckResult::default();

    if !options.quiet {
        info!("Starting fsck");
    }

    let _guard = if options.no_lock { None } else { Some(filesystem.lock_commits().await) };

    let mut fsck = Fsck::new(options);

    let object_manager = filesystem.object_manager();
    let super_block_header = filesystem.super_block_header();

    // Keep track of all things that might exist in journal checkpoints so we can check for
    // unexpected entries.
    let mut journal_checkpoint_ids: HashSet<u64> = HashSet::default();
    journal_checkpoint_ids.insert(super_block_header.allocator_object_id);
    journal_checkpoint_ids.insert(super_block_header.root_store_object_id);

    // Scan the root parent object store.
    let mut root_objects =
        vec![super_block_header.root_store_object_id, super_block_header.journal_object_id];
    root_objects.append(&mut object_manager.root_store().parent_objects());
    fsck.verbose("Scanning root parent store...");
    store_scanner::scan_store(
        &fsck,
        object_manager.root_parent_store().as_ref(),
        &root_objects,
        &mut result,
    )
    .await?;
    fsck.verbose("Scanning root parent store done");

    let root_store = &object_manager.root_store();
    let mut root_store_root_objects = Vec::new();
    root_store_root_objects.append(&mut vec![
        super_block_header.allocator_object_id,
        SuperBlockInstance::A.object_id(),
        SuperBlockInstance::B.object_id(),
    ]);
    root_store_root_objects.append(&mut root_store.root_objects());

    let root_volume = root_volume(filesystem.clone()).await?;
    let volume_directory = root_volume.volume_directory();
    let layer_set = volume_directory.store().tree().layer_set();
    let mut merger = layer_set.merger();
    let mut iter = volume_directory.iter(&mut merger).await?;

    // TODO(https://fxbug.dev/42178153): We could maybe iterate over stores concurrently.
    while let Some((_, store_id, _)) = iter.get() {
        journal_checkpoint_ids.insert(store_id);
        fsck.check_child_store_metadata(
            filesystem.as_ref(),
            store_id,
            &mut root_store_root_objects,
        )
        .await?;
        iter.advance().await?;
    }

    let allocator = filesystem.allocator();
    root_store_root_objects.append(&mut allocator.parent_objects());

    if fsck.options.do_slow_passes {
        // Scan each layer file for the root store.
        let layer_set = root_store.tree().immutable_layer_set();
        fsck.verbose(format!("Checking {} layers for root store...", layer_set.layers.len()));
        for layer in layer_set.layers {
            if let Some(handle) = layer.handle() {
                fsck.verbose(format!(
                    "Layer file {} for root_store is {} bytes",
                    handle.object_id(),
                    handle.get_size()
                ));
            }
            fsck.check_layer_file_contents(
                root_store.store_object_id(),
                layer.handle().map(|h| h.object_id()).unwrap_or(INVALID_OBJECT_ID),
                layer.clone(),
            )
            .await?;
        }

        // Scan each layer file for the allocator.
        let layer_set = allocator.tree().immutable_layer_set();
        fsck.verbose(format!("Checking {} layers for allocator...", layer_set.layers.len()));
        for layer in layer_set.layers {
            if let Some(handle) = layer.handle() {
                fsck.verbose(format!(
                    "Layer file {} for allocator is {} bytes",
                    handle.object_id(),
                    handle.get_size()
                ));
            }
            fsck.check_layer_file_contents(
                allocator.object_id(),
                layer.handle().map(|h| h.object_id()).unwrap_or(INVALID_OBJECT_ID),
                layer.clone(),
            )
            .await?;
        }
        fsck.verbose("Checking layers done");
    }

    // Finally scan the root object store.
    fsck.verbose("Scanning root object store...");
    store_scanner::scan_store(&fsck, root_store.as_ref(), &root_store_root_objects, &mut result)
        .await?;
    fsck.verbose("Scanning root object store done");

    // Now compare our regenerated allocation map with what we actually have.
    fsck.verbose("Verifying allocations...");
    let mut store_ids = HashSet::default();
    store_ids.insert(root_store.store_object_id());
    store_ids.insert(object_manager.root_parent_store().store_object_id());
    fsck.verify_allocations(filesystem.as_ref(), &store_ids, &mut result).await?;
    fsck.verbose("Verifying allocations done");

    // Every key in journal_file_offsets should map to an lsm tree (ObjectStore or Allocator).
    // Excess entries mean we won't be able to reap the journal to free space.
    // Missing entries are OK. Entries only exist if there is data for the store that hasn't been
    // flushed yet.
    for object_id in object_manager.journal_file_offsets().0.keys() {
        if !journal_checkpoint_ids.contains(object_id) {
            fsck.error(FsckError::UnexpectedJournalFileOffset(*object_id))?;
        }
    }

    let errors = fsck.errors();
    let warnings = fsck.warnings();
    if errors > 0 || (fsck.options.fail_on_warning && warnings > 0) {
        Err(anyhow!("Fsck encountered {} errors, {} warnings", errors, warnings))
    } else {
        if warnings > 0 {
            warn!(count = warnings; "Fsck encountered warnings");
        } else {
            if !options.quiet {
                info!("No issues detected");
            }
        }
        Ok(result)
    }
}

/// Verifies the integrity of a volume within Fxfs.  See errors.rs for a list of checks performed.
// TODO(https://fxbug.dev/42178152): This currently takes a write lock on the filesystem.  It would
// be nice if we could take a snapshot.
pub async fn fsck_volume(
    filesystem: &FxFilesystem,
    store_id: u64,
    crypt: Option<Arc<dyn Crypt>>,
) -> Result<FsckResult, Error> {
    fsck_volume_with_options(filesystem, &FsckOptions::default(), store_id, crypt).await
}

pub async fn fsck_volume_with_options(
    filesystem: &FxFilesystem,
    options: &FsckOptions<'_>,
    store_id: u64,
    crypt: Option<Arc<dyn Crypt>>,
) -> Result<FsckResult, Error> {
    let mut result = FsckResult::default();
    if !options.quiet {
        info!(store_id:?; "Starting volume fsck");
    }

    let store = filesystem.object_manager().store(store_id).context("open_store failed").unwrap();

    let _relock_guard;
    if store.is_locked() {
        let crypt = crypt.ok_or_else(|| anyhow!("Invalid key"))?;
        store.unlock_read_only(crypt).await?;
        _relock_guard = scopeguard::guard(store.clone(), |store| {
            store.lock_read_only();
        });
    }

    let _guard = if options.no_lock { None } else { Some(filesystem.lock_commits().await) };

    let mut fsck = Fsck::new(options);
    fsck.check_child_store(store.as_ref(), &mut result).await?;
    let mut store_ids = HashSet::default();
    store_ids.insert(store_id);
    fsck.verify_allocations(filesystem, &store_ids, &mut result).await?;

    let errors = fsck.errors();
    let warnings = fsck.warnings();
    if errors > 0 || (fsck.options.fail_on_warning && warnings > 0) {
        Err(anyhow!("Volume fsck encountered {} errors, {} warnings", errors, warnings))
    } else {
        if warnings > 0 {
            warn!(count = warnings; "Volume fsck encountered warnings");
        } else {
            if !options.quiet {
                info!("No issues detected");
            }
        }
        Ok(result)
    }
}

struct Fsck<'a> {
    options: &'a FsckOptions<'a>,
    // A list of allocations generated based on all extents found across all scanned object stores.
    allocations: Arc<SkipListLayer<AllocatorKey, AllocatorValue>>,
    errors: AtomicU64,
    warnings: AtomicU64,
}

impl<'a> Fsck<'a> {
    fn new(options: &'a FsckOptions<'a>) -> Self {
        Fsck {
            options,
            // TODO(https://fxbug.dev/42178047): fix magic number
            allocations: SkipListLayer::new(2048),
            errors: AtomicU64::new(0),
            warnings: AtomicU64::new(0),
        }
    }

    // Log if in verbose mode.
    fn verbose(&self, message: impl AsRef<str>) {
        if self.options.verbose {
            info!(message = message.as_ref(); "fsck");
        }
    }

    fn errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    fn warnings(&self) -> u64 {
        self.warnings.load(Ordering::Relaxed)
    }

    fn assert<V>(&self, res: Result<V, Error>, error: FsckFatal) -> Result<V, Error> {
        if res.is_err() {
            (self.options.on_error)(&FsckIssue::Fatal(error.clone()));
            return Err(anyhow!("{:?}", error)).context(res.err().unwrap());
        }
        res
    }

    fn warning(&self, error: FsckWarning) -> Result<(), Error> {
        (self.options.on_error)(&FsckIssue::Warning(error));
        self.warnings.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn error(&self, error: FsckError) -> Result<(), Error> {
        (self.options.on_error)(&FsckIssue::Error(error.clone()));
        self.errors.fetch_add(1, Ordering::Relaxed);
        if self.options.halt_on_error { Err(anyhow!("{:?}", error)) } else { Ok(()) }
    }

    fn fatal(&self, error: FsckFatal) -> Result<(), Error> {
        (self.options.on_error)(&FsckIssue::Fatal(error.clone()));
        Err(anyhow!("{:?}", error))
    }

    // Does not actually verify the inner contents of the store; for that, use check_child_store.
    async fn check_child_store_metadata(
        &mut self,
        filesystem: &FxFilesystem,
        store_id: u64,
        root_store_root_objects: &mut Vec<u64>,
    ) -> Result<(), Error> {
        let root_store = filesystem.root_store();

        // Manually open the StoreInfo so we can validate it without unlocking the store.
        let info = self.assert(
            load_store_info(&root_store, store_id).await,
            FsckFatal::MalformedStore(store_id),
        )?;
        root_store_root_objects.append(&mut info.parent_objects());
        Ok(())
    }

    async fn check_child_store(
        &mut self,
        store: &ObjectStore,
        result: &mut FsckResult,
    ) -> Result<(), Error> {
        let store_id = store.store_object_id();
        if self.options.do_slow_passes {
            let layer_set = store.tree().immutable_layer_set();
            for layer in layer_set.layers {
                let (layer_object_id, layer_size) = if let Some(h) = layer.handle() {
                    (h.object_id(), h.get_size())
                } else {
                    (0, 0)
                };
                self.verbose(format!(
                    "Layer file {} for store {} is {} bytes",
                    layer_object_id, store_id, layer_size,
                ));
                self.check_layer_file_contents(store_id, layer_object_id, layer.clone()).await?
            }
        }

        store_scanner::scan_store(self, store, &store.root_objects(), result)
            .await
            .context("scan_store failed")
    }

    async fn check_layer_file_contents<K: Key + LayerKey, V: Value>(
        &self,
        // This is the object ID of the store or allocator that the layer files belong to.
        allocator_or_store_object_id: u64,
        layer_file_object_id: u64,
        layer: Arc<dyn Layer<K, V>>,
    ) -> Result<(), Error> {
        let mut iter: BoxedLayerIterator<'_, K, V> = self.assert(
            layer.seek(Bound::Unbounded).await,
            FsckFatal::MalformedLayerFile(allocator_or_store_object_id, layer_file_object_id),
        )?;

        let mut last_item: Option<Item<K, V>> = None;
        while let Some(item) = iter.get() {
            if let Some(last) = last_item {
                if !last.key.cmp_upper_bound(&item.key).is_le() {
                    self.fatal(FsckFatal::MisOrderedLayerFile(
                        allocator_or_store_object_id,
                        layer_file_object_id,
                    ))?;
                }
                if last.key.overlaps(&item.key) {
                    self.fatal(FsckFatal::OverlappingKeysInLayerFile(
                        allocator_or_store_object_id,
                        layer_file_object_id,
                        item.into(),
                        last.as_item_ref().into(),
                    ))?;
                }
            }
            if layer.maybe_contains_key(item.key) == MaybeContainsKey::False {
                // Key reported as not existing in filter
                self.fatal(FsckFatal::InvalidBloomFilter(
                    allocator_or_store_object_id,
                    layer_file_object_id,
                    item.into(),
                ))?;
            }
            last_item = Some(item.cloned());
            self.assert(
                iter.advance().await,
                FsckFatal::MalformedLayerFile(allocator_or_store_object_id, layer_file_object_id),
            )?;
        }
        Ok(())
    }

    // Assumes that every store in `store_object_ids` has been previously scanned.
    async fn verify_allocations(
        &self,
        filesystem: &FxFilesystem,
        store_object_ids: &HashSet<u64>,
        result: &mut FsckResult,
    ) -> Result<(), Error> {
        let allocator = filesystem.allocator();
        let layer_set = allocator.tree().layer_set();
        let mut merger = layer_set.merger();
        let mut stored_allocations = CoalescingIterator::new(
            allocator.filter(merger.query(Query::FullScan).await?, true).await?,
        )
        .await
        .expect("filter failed");
        let mut observed_allocations =
            CoalescingIterator::new(self.allocations.seek(Bound::Unbounded)).await?;
        let mut observed_owner_allocated_bytes = BTreeMap::new();
        let mut extra_allocations: Vec<errors::Allocation> = vec![];
        let bs = filesystem.block_size();
        let mut previous_allocation_end = 0;
        while let Some(allocation) = stored_allocations.get() {
            let r = &allocation.key.device_range;
            if !bs.is_aligned(r) {
                self.error(FsckError::MisalignedAllocation(allocation.into()))?;
            } else if r.start >= r.end {
                self.error(FsckError::MalformedAllocation(allocation.into()))?;
            }
            let owner_object_id = match allocation.value {
                AllocatorValue::None => INVALID_OBJECT_ID,
                AllocatorValue::Abs { owner_object_id, .. } => *owner_object_id,
            };

            // 'None' allocator values represent free space so should be ignored here.
            if allocation.value != &AllocatorValue::None {
                if r.start > previous_allocation_end {
                    let size = r.start - previous_allocation_end;
                    result.fragmentation.free_space
                        [FragmentationStats::get_histogram_bucket_for_size(size)] += 1;
                }
                previous_allocation_end = r.end;
            }

            *observed_owner_allocated_bytes.entry(owner_object_id).or_insert(0) += r.end - r.start;
            if !store_object_ids.contains(&owner_object_id) {
                if filesystem.object_manager().store(owner_object_id).is_none() {
                    self.error(FsckError::AllocationForNonexistentOwner(allocation.into()))?;
                }
                stored_allocations.advance().await?;
                continue;
            }
            // Cross-reference allocations against the ones we observed.
            match observed_allocations.get() {
                None => extra_allocations.push(allocation.into()),
                Some(observed_allocation) => {
                    if allocation.key.device_range.end <= observed_allocation.key.device_range.start
                    {
                        extra_allocations.push(allocation.into());
                        stored_allocations.advance().await?;
                        continue;
                    }
                    if observed_allocation.key.device_range.end <= allocation.key.device_range.start
                    {
                        self.error(FsckError::MissingAllocation(observed_allocation.into()))?;
                        observed_allocations.advance().await?;
                        continue;
                    }
                    // We can only reconstruct the key/value fields of Item.
                    if allocation.key != observed_allocation.key
                        || allocation.value != observed_allocation.value
                    {
                        self.error(FsckError::AllocationMismatch(
                            observed_allocation.into(),
                            allocation.into(),
                        ))?;
                        stored_allocations.advance().await?;
                        continue;
                    }
                }
            }
            try_join!(stored_allocations.advance(), observed_allocations.advance())?;
        }
        let device_size =
            filesystem.device().block_count() * filesystem.device().block_size() as u64;
        if previous_allocation_end < device_size {
            let size = device_size - previous_allocation_end;
            result.fragmentation.free_space
                [FragmentationStats::get_histogram_bucket_for_size(size)] += 1;
        }
        while let Some(allocation) = observed_allocations.get() {
            self.error(FsckError::MissingAllocation(allocation.into()))?;
            observed_allocations.advance().await?;
            continue;
        }
        let expected_allocated_bytes = observed_owner_allocated_bytes.values().sum::<u64>();
        self.verbose(format!(
            "Found {} bytes allocated (expected {} bytes). Total device size is {} bytes.",
            allocator.get_allocated_bytes(),
            expected_allocated_bytes,
            device_size,
        ));
        if !extra_allocations.is_empty() {
            self.error(FsckError::ExtraAllocations(extra_allocations))?;
        }
        // NB: If the allocator returns a value of 0 for a store, it just means the store has no
        // data that it owns.  Fsck wouldn't have observed any allocations for these, so filter them
        // out.
        let owner_allocated_bytes = allocator
            .get_owner_allocated_bytes()
            .into_iter()
            .filter(|(_, v)| *v > 0)
            .collect::<BTreeMap<_, _>>();
        if expected_allocated_bytes != allocator.get_allocated_bytes()
            || observed_owner_allocated_bytes.len() != owner_allocated_bytes.len()
            || zip(observed_owner_allocated_bytes.iter(), owner_allocated_bytes.iter())
                .filter(|((k1, v1), (k2, v2))| (*k1, *v1) != (*k2, *v2))
                .count()
                != 0
        {
            self.error(FsckError::AllocatedBytesMismatch(
                observed_owner_allocated_bytes.iter().map(|(k, v)| (*k, *v)).collect(),
                owner_allocated_bytes.iter().map(|(k, v)| (*k, *v)).collect(),
            ))?;
        }
        for (k, v) in allocator.owner_byte_limits() {
            if !owner_allocated_bytes.contains_key(&k) {
                self.warning(FsckWarning::LimitForNonExistentStore(k, v))?;
            }
        }
        Ok(())
    }
}
