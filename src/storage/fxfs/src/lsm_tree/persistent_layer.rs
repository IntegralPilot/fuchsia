// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// PersistentLayer object format
//
// The layer is made up of 1 or more "blocks" whose size are some multiple of the block size used
// by the underlying handle.
//
// The persistent layer has 4 types of blocks:
//  - Header block
//  - Data block
//  - BloomFilter block
//  - Seek block (+LayerInfo)
//
// The structure of the file is as follows:
//
// blk#     contents
// 0        [Header]
// 1        [Data]
// 2        [Data]
// ...      [Data]
// L        [BloomFilter]
// L + 1    [BloomFilter]
// ...      [BloomFilter]
// M        [Seek]
// M + 1    [Seek]
// ...      [Seek]
// N        [Seek/LayerInfo]
//
// Generally, there will be an order of magnitude more Data blocks than Seek/BloomFilter blocks.
//
// Header contains a Version-prefixed LayerHeader struct.  This version is used for everything in
// the layer file.
//
// Data blocks contain a little endian encoded u16 item count at the start, then a series of
// serialized items, and a list of little endian u16 offsets within the block for where
// serialized items start, excluding the first item (since it is at a known offset). The list of
// offsets ends at the end of the block and since the items are of variable length, there may be
// space between the two sections if the next item and its offset cannot fit into the block.
//
// |item_count|item|item|item|item|item|item|dead space|offset|offset|offset|offset|offset|
//
// BloomFilter blocks contain a bitmap which is used to probabilistically determine if a given key
// might exist in the layer file.   See `BloomFilter` for details on this structure.  Note that this
// can be absent from the file for small layer files.
//
// Seek/LayerInfo blocks contain both the seek table, and a single LayerInfo struct at the tail of
// the last block, with the LayerInfo's length written as a little-endian u64 at the very end.  The
// padding between the two structs is ignored but nominally is zeroed. They share blocks to avoid
// wasting padding bytes.  Note that the seek table can be absent from the file for small layer
// files (but there will always be one block for the LayerInfo).
//
// The seek table consists of a little-endian u64 for every data block except for the first one. The
// entries should be monotonically increasing, as they represent some mapping for how the keys for
// the first item in each block would be predominantly sorted, and there may be duplicate entries.
// There should be exactly as many seek blocks as are required to house one entry fewer than the
// number of data blocks.

use crate::drop_event::DropEvent;
use crate::errors::FxfsError;
use crate::filesystem::MAX_BLOCK_SIZE;
use crate::log::*;
use crate::lsm_tree::bloom_filter::{BloomFilterReader, BloomFilterStats, BloomFilterWriter};
use crate::lsm_tree::types::{
    BoxedLayerIterator, Existence, FuzzyHash, Item, ItemRef, Key, Layer, LayerIterator, LayerValue,
    LayerWriter, MaybeContainsKey,
};
use crate::object_handle::{ObjectHandle, ReadObjectHandle, WriteBytes};
use crate::object_store::caching_object_handle::{CHUNK_SIZE, CachedChunk, CachingObjectHandle};
use crate::object_store::extent::MIN_BLOCK_SIZE;
use crate::serialized_types::{LATEST_VERSION, Version, Versioned, VersionedLatest};
use anyhow::{Context, Error, anyhow, bail, ensure};
use async_trait::async_trait;
use byteorder::{ByteOrder, LittleEndian, ReadBytesExt, WriteBytesExt};
use fprint::TypeFingerprint;
use fuchsia_sync::Mutex;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, TryStreamExt};
use serde::{Deserialize, Serialize};
use static_assertions::const_assert;
use std::cmp::Ordering;
use std::io::{Read, Write as _};
use std::marker::PhantomData;
use std::ops::Bound;
use std::sync::Arc;
use storage_units::BlockSize;

const PERSISTENT_LAYER_MAGIC: &[u8; 8] = b"FxfsLayr";

/// LayerHeader is stored in the first block of the persistent layer.
pub type LayerHeader = LayerHeaderV39;

#[derive(Debug, Serialize, Deserialize, TypeFingerprint, Versioned)]
pub struct LayerHeaderV39 {
    /// 'FxfsLayr'
    magic: [u8; 8],
    /// The block size used within this layer file. This is typically set at compaction time to the
    /// same block size as the underlying object handle.
    ///
    /// (Each block starts with a 2 byte item count so there is a 64k item limit per block,
    /// regardless of block size).
    block_size: u64,
}

/// The last block of each layer contains metadata for the rest of the layer.
pub type LayerInfo = LayerInfoV39;

#[derive(Debug, Serialize, Deserialize, TypeFingerprint, Versioned)]
pub struct LayerInfoV39 {
    /// How many items are in the layer file.  Mainly used for sizing bloom filters during
    /// compaction.
    num_items: usize,
    /// The number of data blocks in the layer file.
    num_data_blocks: u64,
    /// The size of the bloom filter in the layer file.  Not necessarily block-aligned.
    bloom_filter_size_bytes: usize,
    /// The seed for the nonces used in the bloom filter.
    bloom_filter_seed: u64,
    /// How many nonces to use for bloom filter hashing.
    bloom_filter_num_hashes: usize,
}

/// A handle to a persistent layer.
pub struct PersistentLayer<K, V> {
    // We retain a reference to the underlying object handle so we can hand out references to it for
    // `Layer::handle` when clients need it.  Internal reads should go through
    // `caching_object_handle` so they are cached.  Note that `CachingObjectHandle` used to
    // implement `ReadObjectHandle`, but that was removed so that `CachingObjectHandle` could hand
    // out data references rather than requiring copying to a buffer, which speeds up LSM tree
    // operations.
    object_handle: Arc<dyn ReadObjectHandle>,
    caching_object_handle: CachingObjectHandle<Arc<dyn ReadObjectHandle>>,
    version: Version,
    block_size: BlockSize,
    data_size: u64,
    seek_table: Vec<u64>,
    num_items: usize,
    bloom_filter: Option<BloomFilterReader<K>>,
    bloom_filter_stats: Option<BloomFilterStats>,
    close_event: Mutex<Option<Arc<DropEvent>>>,
    _value_type: PhantomData<V>,
}

#[derive(Debug)]
struct BufferCursor {
    chunk: Option<CachedChunk>,
    pos: usize,
}

impl std::io::Read for BufferCursor {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let chunk = if let Some(chunk) = &self.chunk {
            chunk
        } else {
            return Ok(0);
        };
        let to_read = std::cmp::min(buf.len(), chunk.len().saturating_sub(self.pos));
        if to_read > 0 {
            buf[..to_read].copy_from_slice(&chunk[self.pos..self.pos + to_read]);
            self.pos += to_read;
        }
        Ok(to_read)
    }
}

// For small layer files, don't bother with the bloom filter.  Arbitrarily chosen.
const MINIMUM_DATA_BLOCKS_FOR_BLOOM_FILTER: usize = 4;

// How many blocks we reserve for the header.  Data blocks start at this offset.
const NUM_HEADER_BLOCKS: u64 = 1;

/// The smallest possible (empty) layer file is always 2 blocks, one for the header and one for
/// LayerInfo.
const MINIMUM_LAYER_FILE_BLOCKS: u64 = 2;

// Put safety rails on the size of the bloom filter and seek table to avoid OOMing the system.
// It's more likely that tampering has occurred in these cases.
const MAX_BLOOM_FILTER_SIZE: usize = 64 * 1024 * 1024;
const MAX_SEEK_TABLE_SIZE: usize = 64 * 1024 * 1024;

// The following constants refer to sizes of metadata in the data blocks.
const PER_DATA_BLOCK_HEADER_SIZE: usize = 2;
const PER_DATA_BLOCK_SEEK_ENTRY_SIZE: usize = 2;

// A key-only iterator, used while seeking through the tree.
struct KeyOnlyIterator<'iter, K: Key, V: LayerValue> {
    // Allocated out of |layer|.
    buffer: BufferCursor,

    layer: &'iter PersistentLayer<K, V>,

    // The position of the _next_ block to be read.
    pos: u64,

    // The item index in the current block.
    item_index: u16,

    // The number of items in the current block.
    item_count: u16,

    // The current key.
    key: Option<K>,

    // Set by a wrapping iterator once the value has been deserialized, so the KeyOnlyIterator knows
    // whether it is pointing at the next key or not.
    value_deserialized: bool,
}

impl<K: Key, V: LayerValue> KeyOnlyIterator<'_, K, V> {
    fn new<'iter>(layer: &'iter PersistentLayer<K, V>, pos: u64) -> KeyOnlyIterator<'iter, K, V> {
        assert!(layer.block_size.is_aligned(pos));
        KeyOnlyIterator {
            layer,
            buffer: BufferCursor { chunk: None, pos: (pos % CHUNK_SIZE) as usize },
            pos,
            item_index: 0,
            item_count: 0,
            key: None,
            value_deserialized: false,
        }
    }

    // Repositions the iterator to point to the `index`'th item in the current block.
    // Returns an error if the index is out of range or the resulting offset contains an obviously
    // invalid value.
    fn seek_to_block_item(&mut self, index: u16) -> Result<(), Error> {
        ensure!(index < self.item_count, FxfsError::OutOfRange);
        if index == self.item_index && self.value_deserialized {
            // Fast-path when we are seeking in a linear manner, as is the case when advancing a
            // wrapping iterator that also deserializes the values.
            return Ok(());
        }
        let offset_in_block = if index == 0 {
            // First entry isn't actually recorded, it is at the start of the block after the item
            // count.
            PER_DATA_BLOCK_HEADER_SIZE
        } else {
            let old_buffer_pos = self.buffer.pos;
            self.buffer.pos = self.layer.block_size.align_up(self.buffer.pos as u64).unwrap()
                as usize
                - (PER_DATA_BLOCK_SEEK_ENTRY_SIZE * (usize::from(self.item_count - index)));
            let res = self.buffer.read_u16::<LittleEndian>();
            self.buffer.pos = old_buffer_pos;
            let offset_in_block = res.context("Failed to read offset")? as usize;
            if offset_in_block >= self.layer.block_size.get() as usize
                || offset_in_block <= PER_DATA_BLOCK_HEADER_SIZE
            {
                return Err(anyhow!(FxfsError::Inconsistent))
                    .context(format!("Offset {} is out of valid range.", offset_in_block));
            }
            offset_in_block
        };
        self.item_index = index;
        self.buffer.pos =
            (self.layer.block_size.align_down(self.buffer.pos as u64) as usize) + offset_in_block;
        Ok(())
    }

    async fn advance(&mut self) -> Result<(), Error> {
        if self.item_index >= self.item_count {
            if self.pos >= self.layer.data_offset() + self.layer.data_size {
                self.key = None;
                return Ok(());
            }
            if self.buffer.chunk.is_none() || CHUNK_SIZE.is_aligned(self.pos) {
                self.buffer.chunk = Some(
                    self.layer
                        .caching_object_handle
                        .read(self.pos as usize)
                        .await
                        .context("Reading during advance")?,
                );
            }
            self.buffer.pos = (self.pos % CHUNK_SIZE) as usize;
            self.item_count = self.buffer.read_u16::<LittleEndian>()?;
            if self.item_count == 0 {
                bail!(
                    "Read block with zero item count (object: {}, offset: {})",
                    self.layer.object_handle.object_id(),
                    self.pos
                );
            }
            debug!(
                pos = self.pos,
                buf:? = self.buffer,
                object_size = self.layer.data_offset() + self.layer.data_size,
                oid = self.layer.object_handle.object_id();
                ""
            );
            self.pos += self.layer.block_size;
            self.item_index = 0;
            self.value_deserialized = true;
        }
        self.seek_to_block_item(self.item_index)?;
        self.key = Some(
            K::deserialize_from_version(self.buffer.by_ref(), self.layer.version)
                .context("Corrupt layer (key)")?,
        );
        self.item_index += 1;
        self.value_deserialized = false;
        Ok(())
    }

    fn try_advance(&mut self) -> Result<bool, Error> {
        if self.item_index >= self.item_count {
            if self.pos >= self.layer.data_offset() + self.layer.data_size {
                self.key = None;
                return Ok(true);
            }
            if self.buffer.chunk.is_none() || CHUNK_SIZE.is_aligned(self.pos) {
                self.buffer.chunk = self.layer.caching_object_handle.try_read(self.pos as usize);
                if self.buffer.chunk.is_none() {
                    return Ok(false);
                }
            }
            self.buffer.pos = (self.pos % CHUNK_SIZE) as usize;
            self.item_count = self.buffer.read_u16::<LittleEndian>()?;
            if self.item_count == 0 {
                bail!(
                    "Read block with zero item count (object: {}, offset: {})",
                    self.layer.object_handle.object_id(),
                    self.pos
                );
            }
            debug!(
                pos = self.pos,
                buf:? = self.buffer,
                object_size = self.layer.data_offset() + self.layer.data_size,
                oid = self.layer.object_handle.object_id();
                ""
            );
            self.pos += self.layer.block_size;
            self.item_index = 0;
            self.value_deserialized = true;
        }
        self.seek_to_block_item(self.item_index)?;
        self.key = Some(
            K::deserialize_from_version(self.buffer.by_ref(), self.layer.version)
                .context("Corrupt layer (key)")?,
        );
        self.item_index += 1;
        self.value_deserialized = false;
        Ok(true)
    }

    fn get(&self) -> Option<&K> {
        self.key.as_ref()
    }

    fn take_item(&mut self) -> Result<Option<Item<K, V>>, Error> {
        let key = std::mem::take(&mut self.key);
        if let Some(key) = key {
            self.value_deserialized = true;
            let value = V::deserialize_from_version(self.buffer.by_ref(), self.layer.version)
                .context("Corrupt layer (value)")?;
            Ok(Some(Item { key, value }))
        } else {
            Ok(None)
        }
    }
}

struct Iterator<'iter, K: Key, V: LayerValue> {
    inner: KeyOnlyIterator<'iter, K, V>,
    // The current item.
    item: Option<Item<K, V>>,
}

impl<'iter, K: Key, V: LayerValue> Iterator<'iter, K, V> {
    fn new(mut seek_iterator: KeyOnlyIterator<'iter, K, V>) -> Result<Self, Error> {
        let item = seek_iterator.take_item()?;
        Ok(Self { inner: seek_iterator, item })
    }
}

impl<'iter, K: Key, V: LayerValue> LayerIterator<K, V> for Iterator<'iter, K, V> {
    async fn advance(&mut self) -> Result<(), Error> {
        self.inner.advance().await?;
        self.item = self.inner.take_item()?;
        Ok(())
    }

    fn advance_dyn<'a>(&'a mut self) -> Result<Option<BoxFuture<'a, Result<(), Error>>>, Error> {
        if self.inner.try_advance()? {
            self.item = self.inner.take_item()?;
            Ok(None)
        } else {
            Ok(Some(Box::pin(self.advance())))
        }
    }

    fn get(&self) -> Option<ItemRef<'_, K, V>> {
        self.item.as_ref().map(<&Item<K, V>>::into)
    }
}

// Returns the size of the seek table in bytes.
fn seek_table_size(num_data_blocks: u64) -> usize {
    // The first data block doesn't have an entry.
    let seek_table_entries = num_data_blocks.saturating_sub(1) as usize;
    if seek_table_entries == 0 {
        return 0;
    }
    let entry_size = std::mem::size_of::<u64>();
    seek_table_entries * entry_size
}

async fn load_seek_table(
    object_handle: &(impl ReadObjectHandle + 'static),
    seek_table_offset: u64,
    num_data_blocks: u64,
) -> Result<Vec<u64>, Error> {
    let seek_table_size = seek_table_size(num_data_blocks);
    if seek_table_size == 0 {
        return Ok(vec![]);
    }
    if seek_table_size > MAX_SEEK_TABLE_SIZE {
        return Err(anyhow!(FxfsError::NotSupported)).context("Seek table too large");
    }
    let mut buffer = object_handle.allocate_buffer(seek_table_size).await;
    let bytes_read = object_handle
        .read(seek_table_offset, buffer.as_mut())
        .await
        .context("Reading seek table blocks")?;
    ensure!(bytes_read == seek_table_size, "Short read");

    let mut seek_table = Vec::with_capacity(num_data_blocks as usize);
    // No entry for the first data block, assume a lower bound 0.
    seek_table.push(0);
    let mut prev = 0;
    for chunk in buffer.subslice(0..seek_table_size).as_ptr_slice().iter_as::<[u8; 8]>() {
        let next = u64::from_le_bytes(chunk);
        // Should be in strict ascending order, otherwise something's broken, or we've gone off
        // the end and we're reading zeroes.
        if prev > next {
            return Err(anyhow!(FxfsError::Inconsistent))
                .context(format!("Seek table entry out of order, {:?} > {:?}", prev, next));
        }
        prev = next;
        seek_table.push(next);
    }
    Ok(seek_table)
}

const BLOOM_FILTER_READ_CHUNK_SIZE: usize = 1024 * 1024;

async fn load_bloom_filter<K: FuzzyHash>(
    handle: &(impl ReadObjectHandle + 'static),
    bloom_filter_offset: u64,
    layer_info: &LayerInfo,
) -> Result<Option<BloomFilterReader<K>>, Error> {
    if layer_info.bloom_filter_size_bytes == 0 {
        return Ok(None);
    }
    if layer_info.bloom_filter_size_bytes > MAX_BLOOM_FILTER_SIZE {
        return Err(anyhow!(FxfsError::NotSupported)).context("Bloom filter too large");
    }
    let mut buffer = handle.allocate_buffer(layer_info.bloom_filter_size_bytes).await;
    let reads = FuturesUnordered::new();
    let mut offset = bloom_filter_offset;
    for chunk in buffer.as_mut().chunks_mut(BLOOM_FILTER_READ_CHUNK_SIZE) {
        let chunk_len = chunk.len() as u64;
        reads.push(async move {
            handle.read(offset, chunk).await.context("Failed to read")?;
            Ok::<(), Error>(())
        });
        offset += chunk_len;
    }
    reads.try_collect::<()>().await?;
    Ok(Some(BloomFilterReader::read(
        buffer.subslice(0..layer_info.bloom_filter_size_bytes).as_ptr_slice(),
        layer_info.bloom_filter_seed,
        layer_info.bloom_filter_num_hashes,
    )?))
}

impl<K: Key, V: LayerValue> PersistentLayer<K, V> {
    pub async fn open(handle: impl ReadObjectHandle + 'static) -> Result<Arc<Self>, Error> {
        let handle_block_size = handle.block_size();
        let mut buffer = handle.allocate_buffer(handle_block_size.get() as usize).await;
        handle.read(0, buffer.as_mut()).await.context("Failed to read first block")?;
        let mut reader = buffer.as_ptr_slice();
        let version = Version::deserialize_from(&mut reader)?;

        ensure!(version <= LATEST_VERSION, FxfsError::InvalidVersion);
        let header = LayerHeader::deserialize_from_version(&mut reader, version)
            .context("Failed to deserialize header")?;
        if &header.magic != PERSISTENT_LAYER_MAGIC {
            return Err(anyhow!(FxfsError::Inconsistent).context("Invalid layer file magic"));
        }
        let block_size = BlockSize::from_u64(header.block_size).ok_or_else(|| {
            anyhow!(FxfsError::Inconsistent)
                .context(format!("Invalid block size {}", header.block_size))
        })?;
        ensure!(block_size <= MAX_BLOCK_SIZE, FxfsError::NotSupported);
        ensure!(block_size >= MIN_BLOCK_SIZE, FxfsError::NotSupported);
        if !handle_block_size.is_aligned(block_size.get()) {
            return Err(anyhow!(FxfsError::Inconsistent)).context(format!(
                "{} not a multiple of handle block size {}",
                block_size, handle_block_size
            ));
        }

        if handle.get_size() < MINIMUM_LAYER_FILE_BLOCKS * block_size {
            return Err(anyhow!(FxfsError::Inconsistent).context("Layer file too short"));
        }

        let bs = block_size.get() as usize;
        let layer_info = {
            let last_block_offset = handle
                .get_size()
                .checked_sub(block_size.get())
                .ok_or(FxfsError::Inconsistent)
                .context("Layer file unexpectedly short")?;
            handle
                .read(last_block_offset, buffer.subslice_mut(0..bs))
                .await
                .context("Failed to read layer info")?;
            let layer_info_len =
                u64::from_le_bytes(buffer.subslice(bs - 8..bs).as_ptr_slice().read().unwrap());
            let layer_info_offset = bs
                .checked_sub(std::mem::size_of::<u64>() + layer_info_len as usize)
                .ok_or(FxfsError::Inconsistent)
                .context("Invalid layer info length")?;
            let mut reader = buffer.subslice(layer_info_offset..).as_ptr_slice();
            LayerInfo::deserialize_from_version(&mut reader, version)
                .context("Failed to deserialize LayerInfo")?
        };
        std::mem::drop(buffer);
        if layer_info.num_items == 0 && layer_info.num_data_blocks > 0 {
            return Err(anyhow!(FxfsError::Inconsistent))
                .context("Invalid num_items/num_data_blocks");
        }
        let total_blocks = handle.get_size() / block_size;
        let bloom_filter_blocks =
            block_size.align_up_to_blocks(layer_info.bloom_filter_size_bytes as u64);
        if layer_info.num_data_blocks + bloom_filter_blocks
            > total_blocks - MINIMUM_LAYER_FILE_BLOCKS
        {
            return Err(anyhow!(FxfsError::Inconsistent)).context("Invalid number of blocks");
        }

        let bloom_filter_offset = block_size * (NUM_HEADER_BLOCKS + layer_info.num_data_blocks);
        let bloom_filter = if version == LATEST_VERSION {
            load_bloom_filter(&handle, bloom_filter_offset, &layer_info)
                .await
                .context("Failed to load bloom filter")?
        } else {
            // Ignore the bloom filter for layer files in outdated versions.  We don't know whether
            // keys have changed formats or not (and therefore have different hash values), so we
            // must ignore the bloom filter and always query the layer.
            None
        };
        let bloom_filter_stats = bloom_filter.as_ref().map(|b| b.stats());

        let seek_offset =
            block_size * (NUM_HEADER_BLOCKS + layer_info.num_data_blocks + bloom_filter_blocks);
        let seek_table = load_seek_table(&handle, seek_offset, layer_info.num_data_blocks)
            .await
            .context("Failed to load seek table")?;

        let object_handle = Arc::new(handle) as Arc<dyn ReadObjectHandle>;
        let caching_object_handle = CachingObjectHandle::new(object_handle.clone());
        Ok(Arc::new(PersistentLayer {
            object_handle,
            caching_object_handle,
            version,
            block_size,
            data_size: block_size * layer_info.num_data_blocks,
            seek_table,
            num_items: layer_info.num_items,
            bloom_filter,
            bloom_filter_stats,
            close_event: Mutex::new(Some(Arc::new(DropEvent::new()))),
            _value_type: PhantomData::default(),
        }))
    }

    /// Whether the bloom filter for the layer file is consulted or not.  If this is false, then
    /// `maybe_contains_key` will always return true.
    /// Note that the persistent layer file may still have a bloom filter, but it might be ignored
    /// (e.g. for a layer file on an older version).
    pub fn has_bloom_filter(&self) -> bool {
        self.bloom_filter.is_some()
    }

    fn data_offset(&self) -> u64 {
        NUM_HEADER_BLOCKS * self.block_size
    }

    /// Seeks to `bound`.
    ///
    /// This is an inherent version of the `Layer::seek` trait method which avoids boxing the
    /// returned `Iterator`.
    async fn seek<'a>(&'a self, bound: Bound<&K>) -> Result<Iterator<'a, K, V>, Error> {
        let (key, excluded) = match bound {
            Bound::Unbounded => {
                let mut iterator = Iterator::new(KeyOnlyIterator::new(self, self.data_offset()))?;
                iterator.advance().await.context("Unbounded seek advance")?;
                return Ok(iterator);
            }
            Bound::Included(k) => (k, false),
            Bound::Excluded(k) => (k, true),
        };
        let first_data_block_index = self.data_offset() / self.block_size;

        let (mut left_offset, mut right_offset) = {
            // We are searching for a range here, as multiple items can have the same value in
            // this approximate search. Since the values used are the smallest in the associated
            // block it means that if the value equals the target you should also search the
            // one before it. The goal is for table[left] < target < table[right].
            let target = key.get_leading_u64();
            // Because the first entry in the table is always 0, right_index will never be 0.
            let right_index = self.seek_table.as_slice().partition_point(|&x| x <= target) as u64;
            // Since partition_point will find the index of the first place where the predicate
            // is false, we subtract 1 to get the index where it was last true.
            let left_index = self.seek_table.as_slice()[..right_index as usize]
                .partition_point(|&x| x < target)
                .saturating_sub(1) as u64;

            (
                (left_index + first_data_block_index) * self.block_size,
                (right_index + first_data_block_index) * self.block_size,
            )
        };
        let mut left = KeyOnlyIterator::new(self, left_offset);
        left.advance().await.context("Initial seek advance")?;
        match left.get() {
            None => return Ok(Iterator::new(left)?),
            Some(left_key) => match left_key.cmp_upper_bound(key) {
                Ordering::Greater => return Ok(Iterator::new(left)?),
                Ordering::Equal => {
                    if excluded {
                        left.advance().await?;
                    }
                    return Ok(Iterator::new(left)?);
                }
                Ordering::Less => {}
            },
        }
        let mut right = None;
        while right_offset - left_offset > self.block_size {
            // Pick a block midway.
            let mid_offset =
                self.block_size.align_down(left_offset + (right_offset - left_offset) / 2);
            let mut iterator = KeyOnlyIterator::new(self, mid_offset);
            iterator.advance().await?;
            let iter_key: &K = iterator.get().unwrap();
            match iter_key.cmp_upper_bound(key) {
                Ordering::Greater => {
                    right_offset = mid_offset;
                    right = Some(iterator);
                }
                Ordering::Equal => {
                    if excluded {
                        iterator.advance().await?;
                    }
                    return Ok(Iterator::new(iterator)?);
                }
                Ordering::Less => {
                    left_offset = mid_offset;
                    left = iterator;
                }
            }
        }

        // Finish the binary search on the block pointed to by `left`.
        let mut left_index = 0;
        let mut right_index = left.item_count;
        // If the size is zero then we don't touch the iterator.
        while left_index < (right_index - 1) {
            let mid_index = left_index + ((right_index - left_index) / 2);
            left.seek_to_block_item(mid_index).context("Read index offset for binary search")?;
            left.advance().await?;
            match left.get().unwrap().cmp_upper_bound(key) {
                Ordering::Greater => {
                    right_index = mid_index;
                }
                Ordering::Equal => {
                    if excluded {
                        left.advance().await?;
                    }
                    return Ok(Iterator::new(left)?);
                }
                Ordering::Less => {
                    left_index = mid_index;
                }
            }
        }
        // When we don't find an exact match, we need to return with the first entry *after* the the
        // target key which might be the first one in the next block, currently already pointed to
        // by the "right" buffer, but usually it's just the result of the right index within the
        // "left" buffer.
        if right_index < left.item_count {
            left.seek_to_block_item(right_index)
                .context("Read index for offset of right pointer")?;
        } else if let Some(right) = right {
            return Ok(Iterator::new(right)?);
        } else {
            // We want the end of the layer.  `right_index == left.item_count`, so `left_index ==
            // left.item_count - 1`, and the left iterator must be positioned on `left_index` since
            // we cannot have gone through the `Ordering::Greater` path above because `right_index`
            // would not be equal to `left.item_count` in that case, so all we need to do is advance
            // the iterator.
        }
        left.advance().await?;
        Ok(Iterator::new(left)?)
    }
}

#[async_trait]
impl<K: Key, V: LayerValue> Layer<K, V> for PersistentLayer<K, V> {
    fn handle(&self) -> Option<&dyn ReadObjectHandle> {
        Some(&self.object_handle)
    }

    fn purge_cached_data(&self) {
        self.caching_object_handle.purge();
    }

    fn clear_cached_data(&self) {
        self.caching_object_handle.clear();
    }

    async fn seek<'a>(&'a self, bound: Bound<&K>) -> Result<BoxedLayerIterator<'a, K, V>, Error> {
        Ok(Box::new(PersistentLayer::seek(self, bound).await?))
    }

    fn len(&self) -> usize {
        self.num_items
    }

    fn maybe_contains_key(&self, key: &K) -> MaybeContainsKey {
        self.bloom_filter.as_ref().map_or(MaybeContainsKey::Maybe, |f| f.maybe_contains(key))
    }

    async fn key_exists(&self, key: &K) -> Result<Existence, Error> {
        match &self.bloom_filter {
            Some(filter) => Ok(match filter.maybe_contains(key) {
                MaybeContainsKey::False => Existence::Missing,
                MaybeContainsKey::Maybe | MaybeContainsKey::RangeKeyTooLarge => {
                    Existence::MaybeExists
                }
            }),
            None => {
                let iter = self.seek(Bound::Included(key)).await?;
                Ok(iter.get().map_or(Existence::Missing, |i| {
                    if i.key.cmp_upper_bound(key).is_eq() {
                        Existence::Exists
                    } else {
                        Existence::Missing
                    }
                }))
            }
        }
    }

    fn lock(&self) -> Option<Arc<DropEvent>> {
        self.close_event.lock().clone()
    }

    async fn close(&self) {
        let listener = self.close_event.lock().take().expect("close already called").listen();
        listener.await;
    }

    fn get_version(&self) -> Version {
        return self.version;
    }

    fn record_inspect_data(self: Arc<Self>, node: &fuchsia_inspect::Node) {
        node.record_uint("num_items", self.num_items as u64);
        node.record_bool("persistent", true);
        node.record_uint("size", self.object_handle.get_size());
        if let Some(stats) = self.bloom_filter_stats.as_ref() {
            node.record_child("bloom_filter", move |node| {
                node.record_uint("size", stats.size as u64);
                node.record_uint("num_hashes", stats.num_hashes as u64);
                node.record_uint("fill_percentage", stats.fill_percentage as u64);
            });
        }
    }
}

// This ensures that item_count can't be overflowed below.
const_assert!(MAX_BLOCK_SIZE.size() <= u16::MAX as u64 + 1);

// -- Writer support --

pub struct PersistentLayerWriter<W: WriteBytes, K: Key, V: LayerValue> {
    writer: W,
    block_size: BlockSize,
    buf: Vec<u8>,
    buf_item_count: LayerWriterBufItemCount,
    item_count: usize,
    block_offsets: Vec<u16>,
    block_keys: Vec<u64>,
    bloom_filter: BloomFilterWriter<K>,
    _value: PhantomData<V>,
}

impl<W: WriteBytes, K: Key, V: LayerValue> PersistentLayerWriter<W, K, V> {
    /// Creates a new writer that will serialize items to the object accessible via |object_handle|
    pub async fn new(writer: W, num_items: usize, block_size: BlockSize) -> Result<Self, Error> {
        Self::new_with_version(writer, num_items, block_size, LATEST_VERSION).await
    }

    pub(crate) async fn new_with_version(
        mut writer: W,
        num_items: usize,
        block_size: BlockSize,
        version: Version,
    ) -> Result<Self, Error> {
        ensure!(block_size <= MAX_BLOCK_SIZE, FxfsError::NotSupported);
        ensure!(block_size >= MIN_BLOCK_SIZE, FxfsError::NotSupported);

        // Write the header block.
        let header =
            LayerHeader { magic: PERSISTENT_LAYER_MAGIC.clone(), block_size: block_size.get() };
        let mut buf = vec![0u8; block_size.get() as usize];
        {
            let mut cursor = std::io::Cursor::new(&mut buf[..]);
            version.serialize_into(&mut cursor)?;
            header.serialize_into(&mut cursor)?;
        }
        writer.write_bytes(&buf[..]).await?;

        let seed: u64 = rand::random();
        Ok(Self {
            writer,
            block_size,
            buf: Vec::new(),
            buf_item_count: LayerWriterBufItemCount(0),
            item_count: 0,
            block_offsets: Vec::new(),
            block_keys: Vec::new(),
            bloom_filter: BloomFilterWriter::new(seed, num_items),
            _value: PhantomData,
        })
    }

    /// Writes 'buf[..len]' out as a block.
    ///
    /// Blocks are fixed size, consisting of a 16-bit item count, data, zero padding
    /// and seek table at the end.
    async fn write_block(&mut self, len: usize) -> Result<(), Error> {
        if *self.buf_item_count == 0 {
            return Ok(());
        }
        let seek_table_size = self.block_offsets.len() * PER_DATA_BLOCK_SEEK_ENTRY_SIZE;
        assert!(
            PER_DATA_BLOCK_HEADER_SIZE + seek_table_size + len <= self.block_size.get() as usize
        );
        let mut cursor = std::io::Cursor::new(vec![0u8; self.block_size.get() as usize]);
        cursor.write_u16::<LittleEndian>(*self.buf_item_count)?;
        cursor.write_all(self.buf.drain(..len).as_ref())?;
        cursor.set_position(self.block_size - seek_table_size as u64);
        // Write the seek table. Entries are 2 bytes each and items are always at least 10.
        for &offset in &self.block_offsets {
            cursor.write_u16::<LittleEndian>(offset)?;
        }
        self.writer.write_bytes(cursor.get_ref()).await?;
        debug!(item_count = *self.buf_item_count, byte_count = len; "wrote items");
        *self.buf_item_count = 0;
        self.block_offsets.clear();
        Ok(())
    }

    // Assumes the writer is positioned to a new block.
    // Returns the size, in bytes, of the seek table.
    // Note that the writer will be positioned to exactly the end of the seek table, not to the end
    // of a block.
    async fn write_seek_table(&mut self) -> Result<usize, Error> {
        if self.block_keys.len() == 0 {
            return Ok(0);
        }
        let size = self.block_keys.len() * std::mem::size_of::<u64>();
        self.buf.resize(size, 0);
        let mut len = 0;
        for key in &self.block_keys {
            LittleEndian::write_u64(&mut self.buf[len..len + std::mem::size_of::<u64>()], *key);
            len += std::mem::size_of::<u64>();
        }
        self.writer.write_bytes(&self.buf).await?;
        Ok(size)
    }

    // Assumes the writer is positioned to exactly the end of the seek table, which was
    // `seek_table_len` bytes.
    async fn write_info(
        &mut self,
        num_data_blocks: u64,
        bloom_filter_size_bytes: usize,
        seek_table_len: usize,
    ) -> Result<(), Error> {
        let block_size = self.writer.block_size().get() as usize;
        let layer_info = LayerInfo {
            num_items: self.item_count,
            num_data_blocks,
            bloom_filter_size_bytes,
            bloom_filter_seed: self.bloom_filter.seed(),
            bloom_filter_num_hashes: self.bloom_filter.num_hashes(),
        };
        let actual_len = {
            let mut cursor = std::io::Cursor::new(&mut self.buf);
            layer_info.serialize_into(&mut cursor)?;
            let layer_info_len = cursor.position();
            cursor.write_u64::<LittleEndian>(layer_info_len)?;
            cursor.position() as usize
        };

        // We want the LayerInfo to be at the end of the last block.  That might require creating a
        // new block if we don't have enough room.
        let avail_in_block =
            block_size - (seek_table_len as u64 % self.writer.block_size()) as usize;
        let to_skip = if avail_in_block < actual_len {
            block_size + avail_in_block - actual_len
        } else {
            avail_in_block - actual_len
        } as u64;
        self.writer.skip(to_skip).await?;
        self.writer.write_bytes(&self.buf[..actual_len]).await?;
        Ok(())
    }

    // Assumes the writer is positioned to a new block.
    // Returns the size of the bloom filter, in bytes.
    async fn write_bloom_filter(&mut self) -> Result<usize, Error> {
        if self.data_blocks() < MINIMUM_DATA_BLOCKS_FOR_BLOOM_FILTER {
            return Ok(0);
        }
        // TODO(https://fxbug.dev/323571978): Avoid bounce-buffering.
        let size =
            self.block_size.align_up(self.bloom_filter.serialized_size() as u64).unwrap() as usize;
        self.buf.resize(size, 0);
        let mut cursor = std::io::Cursor::new(&mut self.buf);
        self.bloom_filter.write(&mut cursor)?;
        self.writer.write_bytes(&self.buf).await?;
        Ok(self.bloom_filter.serialized_size())
    }

    // Returns the bloom filter writer. Intended to be used for testing purposes, e.g., gain access
    // to the bloom filter to then corrupt it.
    #[cfg(test)]
    pub(crate) fn bloom_filter(&mut self) -> &mut BloomFilterWriter<K> {
        &mut self.bloom_filter
    }

    fn data_blocks(&self) -> usize {
        if self.item_count == 0 { 0 } else { self.block_keys.len() + 1 }
    }
}

impl<W: WriteBytes + Send, K: Key, V: LayerValue> LayerWriter<K, V>
    for PersistentLayerWriter<W, K, V>
{
    async fn write(&mut self, item: ItemRef<'_, K, V>) -> Result<(), Error> {
        // Note the length before we write this item.
        let len = self.buf.len();
        item.key.serialize_into(&mut self.buf)?;
        item.value.serialize_into(&mut self.buf)?;

        let mut added_offset = false;
        // Never record the first item. The offset is always the same.
        if *self.buf_item_count > 0 {
            self.block_offsets.push(u16::try_from(len + PER_DATA_BLOCK_HEADER_SIZE).unwrap());
            added_offset = true;
        }

        // If writing the item took us over a block, flush the bytes in the buffer prior to this
        // item.
        if PER_DATA_BLOCK_HEADER_SIZE
            + self.buf.len()
            + (self.block_offsets.len() * PER_DATA_BLOCK_SEEK_ENTRY_SIZE)
            > self.block_size.get() as usize - 1
        {
            if added_offset {
                // Drop the recently added offset from the list. The latest item will be the first
                // on the next block and have a known offset there.
                self.block_offsets.pop();
            }
            self.write_block(len).await?;

            // Note that this will not insert an entry for the first data block.
            self.block_keys.push(item.key.get_leading_u64());
        }

        self.bloom_filter.insert(&item.key);
        *self.buf_item_count += 1;
        self.item_count += 1;
        Ok(())
    }

    async fn complete(mut self) -> Result<u64, Error> {
        self.write_block(self.buf.len()).await?;
        let data_blocks = self.data_blocks() as u64;
        let bloom_filter_len = self.write_bloom_filter().await?;
        let seek_table_len = self.write_seek_table().await?;
        self.write_info(data_blocks, bloom_filter_len, seek_table_len).await?;
        self.writer.complete().await
    }
}

/// Logs a warning if this object is dropped and the contained value isn't 0.
#[repr(transparent)]
struct LayerWriterBufItemCount(u16);

impl Drop for LayerWriterBufItemCount {
    fn drop(&mut self) {
        debug_assert!(self.0 == 0, "Dropping unwritten items; did you forget to call complete?");
        if self.0 > 0 {
            warn!("Dropping unwritten items; did you forget to call complete?");
        }
    }
}

impl std::ops::Deref for LayerWriterBufItemCount {
    type Target = u16;
    fn deref(&self) -> &u16 {
        &self.0
    }
}

impl std::ops::DerefMut for LayerWriterBufItemCount {
    fn deref_mut(&mut self) -> &mut u16 {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockSize, PersistentLayer, PersistentLayerWriter};
    use crate::filesystem::MAX_BLOCK_SIZE;
    use crate::lsm_tree::LayerIterator;
    use crate::lsm_tree::persistent_layer::MINIMUM_DATA_BLOCKS_FOR_BLOOM_FILTER;
    use crate::lsm_tree::types::{
        Existence, Item, ItemRef, Layer, LayerWriter, MaybeContainsKey, OrdUpperBound,
    };
    use crate::object_handle::WriteBytes;
    use crate::object_store::AttributeId;
    use crate::object_store::extent::MIN_BLOCK_SIZE;
    use crate::object_store::object_record::ObjectKey;
    use crate::round::round_up;
    use crate::serialized_types::{LATEST_VERSION, Version};
    use crate::testing::fake_object::{FakeObject, FakeObjectHandle};
    use crate::testing::writer::Writer;

    use std::fmt::Debug;

    use std::ops::{Bound, Range};
    use std::sync::Arc;

    impl<W: WriteBytes> Debug for PersistentLayerWriter<W, i32, i32> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
            f.debug_struct("rPersistentLayerWriter")
                .field("block_size", &self.block_size)
                .field("item_count", &*self.buf_item_count)
                .finish()
        }
    }

    #[fuchsia::test]
    async fn test_iterate_after_write() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const ITEM_COUNT: i32 = 10000;

        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                ITEM_COUNT as usize * 4,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            for i in 0..ITEM_COUNT {
                writer.write(Item::new(i, i).as_item_ref()).await.expect("write failed");
            }
            writer.complete().await.expect("flush failed");
        }
        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");
        let mut iterator = layer.seek(Bound::Unbounded).await.expect("seek failed");
        for i in 0..ITEM_COUNT {
            let ItemRef { key, value, .. } = iterator.get().expect("missing item");
            assert_eq!((key, value), (&i, &i));
            iterator.advance().await.expect("failed to advance");
        }
        assert!(iterator.get().is_none());
    }

    #[fuchsia::test]
    async fn test_seek_after_write() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const ITEM_COUNT: i32 = 5000;

        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                ITEM_COUNT as usize * 18,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            for i in 0..ITEM_COUNT {
                // Populate every other value as an item.
                writer.write(Item::new(i * 2, i * 2).as_item_ref()).await.expect("write failed");
            }
            writer.complete().await.expect("flush failed");
        }
        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");
        // Search for all values to check the in-between values.
        for i in 0..ITEM_COUNT * 2 {
            // We've written every other value, we expect to get either the exact value searched
            // for, or the next one after it. So round up to the nearest multiple of 2.
            let expected = round_up(i, 2).unwrap();
            let mut iterator = layer.seek(Bound::Included(&i)).await.expect("failed to seek");
            // We've written values up to (N-1)*2=2*N-2, so when looking for 2*N-1 we'll go off the
            // end of the layer and get back no item.
            if i >= (ITEM_COUNT * 2) - 1 {
                assert!(iterator.get().is_none());
            } else {
                let ItemRef { key, value, .. } = iterator.get().expect("missing item");
                assert_eq!((key, value), (&expected, &expected));
            }

            // Check that we can advance to the next item.
            iterator.advance().await.expect("failed to advance");
            // The highest value is 2*N-2, searching for 2*N-3 will find the last value, and
            // advancing will go off the end of the layer and return no item. If there was
            // previously no item, then it will latch and always return no item.
            if i >= (ITEM_COUNT * 2) - 3 {
                assert!(iterator.get().is_none());
            } else {
                let ItemRef { key, value, .. } = iterator.get().expect("missing item");
                let next = expected + 2;
                assert_eq!((key, value), (&next, &next));
            }
        }
    }

    #[fuchsia::test]
    async fn test_seek_unbounded() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const ITEM_COUNT: i32 = 1000;

        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                ITEM_COUNT as usize * 18,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            for i in 0..ITEM_COUNT {
                writer.write(Item::new(i, i).as_item_ref()).await.expect("write failed");
            }
            writer.complete().await.expect("flush failed");
        }
        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");
        let mut iterator = layer.seek(Bound::Unbounded).await.expect("failed to seek");
        let ItemRef { key, value, .. } = iterator.get().expect("missing item");
        assert_eq!((key, value), (&0, &0));

        // Check that we can advance to the next item.
        iterator.advance().await.expect("failed to advance");
        let ItemRef { key, value, .. } = iterator.get().expect("missing item");
        assert_eq!((key, value), (&1, &1));
    }

    #[fuchsia::test]
    async fn test_zero_items() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;

        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                0,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            writer.complete().await.expect("flush failed");
        }

        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");
        let iterator = (layer.as_ref() as &dyn Layer<i32, i32>)
            .seek(Bound::Unbounded)
            .await
            .expect("seek failed");
        assert!(iterator.get().is_none())
    }

    #[fuchsia::test]
    async fn test_one_item() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;

        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                1,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            writer.write(Item::new(42, 42).as_item_ref()).await.expect("write failed");
            writer.complete().await.expect("flush failed");
        }

        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");
        {
            let mut iterator = (layer.as_ref() as &dyn Layer<i32, i32>)
                .seek(Bound::Unbounded)
                .await
                .expect("seek failed");
            let ItemRef { key, value, .. } = iterator.get().expect("missing item");
            assert_eq!((key, value), (&42, &42));
            iterator.advance().await.expect("failed to advance");
            assert!(iterator.get().is_none())
        }
        {
            let mut iterator = (layer.as_ref() as &dyn Layer<i32, i32>)
                .seek(Bound::Included(&30))
                .await
                .expect("seek failed");
            let ItemRef { key, value, .. } = iterator.get().expect("missing item");
            assert_eq!((key, value), (&42, &42));
            iterator.advance().await.expect("failed to advance");
            assert!(iterator.get().is_none())
        }
        {
            let mut iterator = (layer.as_ref() as &dyn Layer<i32, i32>)
                .seek(Bound::Included(&42))
                .await
                .expect("seek failed");
            let ItemRef { key, value, .. } = iterator.get().expect("missing item");
            assert_eq!((key, value), (&42, &42));
            iterator.advance().await.expect("failed to advance");
            assert!(iterator.get().is_none())
        }
        {
            let iterator = (layer.as_ref() as &dyn Layer<i32, i32>)
                .seek(Bound::Included(&43))
                .await
                .expect("seek failed");
            assert!(iterator.get().is_none())
        }
    }

    #[fuchsia::test]
    async fn test_large_block_size() {
        // At the upper end of the supported size.
        const BLOCK_SIZE: BlockSize = MAX_BLOCK_SIZE;
        // Items will be 18 bytes, so fill up a few pages.
        let item_count: i32 = ((BLOCK_SIZE.get() as i32) / 18) * 3;

        let handle = FakeObjectHandle::new_with_block_size(Arc::new(FakeObject::new()), BLOCK_SIZE);
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                item_count as usize * 18,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            // Use large values to force varint encoding to use consistent space.
            for i in 2000000000..(2000000000 + item_count) {
                writer.write(Item::new(i, i).as_item_ref()).await.expect("write failed");
            }
            writer.complete().await.expect("flush failed");
        }

        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");
        let mut iterator = layer.seek(Bound::Unbounded).await.expect("seek failed");
        for i in 2000000000..(2000000000 + item_count) {
            let ItemRef { key, value, .. } = iterator.get().expect("missing item");
            assert_eq!((key, value), (&i, &i));
            iterator.advance().await.expect("failed to advance");
        }
        assert!(iterator.get().is_none());
    }

    #[fuchsia::test]
    async fn test_overlarge_block_size() {
        // At the upper end of the supported size.
        const BLOCK_SIZE: BlockSize = BlockSize::from_u64(MAX_BLOCK_SIZE.size() * 2).unwrap();

        let handle = FakeObjectHandle::new_with_block_size(Arc::new(FakeObject::new()), BLOCK_SIZE);
        PersistentLayerWriter::<_, i32, i32>::new(Writer::new(&handle).await, 0, BLOCK_SIZE)
            .await
            .expect_err("Creating writer with overlarge block size.");
    }

    #[fuchsia::test]
    async fn test_seek_bound_excluded() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const ITEM_COUNT: i32 = 10000;

        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                ITEM_COUNT as usize * 18,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            for i in 0..ITEM_COUNT {
                writer.write(Item::new(i, i).as_item_ref()).await.expect("write failed");
            }
            writer.complete().await.expect("flush failed");
        }
        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");

        for i in 9982..ITEM_COUNT {
            let mut iterator = layer.seek(Bound::Excluded(&i)).await.expect("failed to seek");
            let i_plus_one = i + 1;
            if i_plus_one < ITEM_COUNT {
                let ItemRef { key, value, .. } = iterator.get().expect("missing item");

                assert_eq!((key, value), (&i_plus_one, &i_plus_one));

                // Check that we can advance to the next item.
                iterator.advance().await.expect("failed to advance");
                let i_plus_two = i + 2;
                if i_plus_two < ITEM_COUNT {
                    let ItemRef { key, value, .. } = iterator.get().expect("missing item");
                    assert_eq!((key, value), (&i_plus_two, &i_plus_two));
                } else {
                    assert!(iterator.get().is_none());
                }
            } else {
                assert!(iterator.get().is_none());
            }
        }
    }

    use crate::lsm_tree::testing::TestKey;

    /// Generates extent records for a given object_id (of size 1).
    /// This produces a series of records with the same leading_u64.
    /// Returns the generated items and the next available object_id.
    fn generate_extents(
        object_id: u64,
        base_offset: u64,
        count: u64,
    ) -> (Vec<Item<ObjectKey, u64>>, u64) {
        let mut items = Vec::new();
        for i in 0..count {
            items.push(Item::new(
                ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    (base_offset + i) * MIN_BLOCK_SIZE..(base_offset + i + 1) * MIN_BLOCK_SIZE,
                ),
                object_id,
            ));
        }
        (items, object_id + 1)
    }

    /// Generates objects object_ids over a range.
    /// This produced a series of records with unique leading_u64.
    /// Returns the generated items and the next available value for sequencing.
    fn generate_objects(object_id_range: Range<u64>) -> (Vec<Item<ObjectKey, u64>>, u64) {
        let mut items = Vec::new();
        let end = object_id_range.end;
        for object_id in object_id_range {
            items.push(Item::new(ObjectKey::object(object_id), object_id));
        }
        (items, end)
    }

    // Create a large spread of data across several blocks to ensure that no part of the range is
    // lost by the partial search using the layer seek table.
    #[fuchsia::test]
    async fn test_block_seek_duplicate_leading_u64() {
        // At the upper end of the supported size.
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const ITEMS_PER_PHASE: u64 = 50;

        let mut to_find = Vec::new();

        let handle = FakeObjectHandle::new_with_block_size(Arc::new(FakeObject::new()), BLOCK_SIZE);
        {
            let mut items = Vec::new();
            // Make all values take up maximum space for varint encoding.
            let mut object_id = u32::MAX as u64 + 1;

            // First fill the front with duplicate object IDs, then look at the start,
            // middle and end of the range.
            {
                let base_extent_offset = 0;
                let (mut generated, next_object_id) =
                    generate_extents(object_id, base_extent_offset, ITEMS_PER_PHASE * 3);
                items.append(&mut generated);
                let count = ITEMS_PER_PHASE * 3;
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    base_extent_offset * MIN_BLOCK_SIZE..(base_extent_offset + 1) * MIN_BLOCK_SIZE,
                ));
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    (base_extent_offset + count / 2) * MIN_BLOCK_SIZE
                        ..(base_extent_offset + count / 2 + 1) * MIN_BLOCK_SIZE,
                ));
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    (base_extent_offset + count - 1) * MIN_BLOCK_SIZE
                        ..(base_extent_offset + count) * MIN_BLOCK_SIZE,
                ));
                object_id = next_object_id;
            }

            // Add some filler of all different leading u64.
            {
                let (mut generated, next_object_id) =
                    generate_objects(object_id..object_id + ITEMS_PER_PHASE * 3);
                items.append(&mut generated);
                object_id = next_object_id;
            }

            // Fill the middle with duplicate object IDs, then look at the start,
            // middle and end of the range.
            {
                let base_extent_offset = 1000;
                let (mut generated, next_object_id) =
                    generate_extents(object_id, base_extent_offset, ITEMS_PER_PHASE * 3);
                items.append(&mut generated);
                let count = ITEMS_PER_PHASE * 3;
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    base_extent_offset * MIN_BLOCK_SIZE..(base_extent_offset + 1) * MIN_BLOCK_SIZE,
                ));
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    (base_extent_offset + count / 2) * MIN_BLOCK_SIZE
                        ..(base_extent_offset + count / 2 + 1) * MIN_BLOCK_SIZE,
                ));
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    (base_extent_offset + count - 1) * MIN_BLOCK_SIZE
                        ..(base_extent_offset + count) * MIN_BLOCK_SIZE,
                ));
                object_id = next_object_id;
            }

            // Add some filler of all different leading u64.
            {
                let (mut generated, next_object_id) =
                    generate_objects(object_id..object_id + ITEMS_PER_PHASE * 3);
                items.append(&mut generated);
                object_id = next_object_id;
            }

            // Fill the end with duplicate object IDs, then look at the start,
            // middle and end of the range.
            {
                let base_extent_offset = 2000;
                let (mut generated, _) =
                    generate_extents(object_id, base_extent_offset, ITEMS_PER_PHASE * 3);
                items.append(&mut generated);
                let count = ITEMS_PER_PHASE * 3;
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    base_extent_offset * MIN_BLOCK_SIZE..(base_extent_offset + 1) * MIN_BLOCK_SIZE,
                ));
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    (base_extent_offset + count / 2) * MIN_BLOCK_SIZE
                        ..(base_extent_offset + count / 2 + 1) * MIN_BLOCK_SIZE,
                ));
                to_find.push(ObjectKey::extent(
                    object_id,
                    AttributeId::TEST_ID,
                    (base_extent_offset + count - 1) * MIN_BLOCK_SIZE
                        ..(base_extent_offset + count) * MIN_BLOCK_SIZE,
                ));
            }

            // Sort items by cmp_upper_bound!
            items.sort_by(|a, b| a.key.cmp_upper_bound(&b.key));

            let mut writer = PersistentLayerWriter::<_, ObjectKey, u64>::new(
                Writer::new(&handle).await,
                (3 * BLOCK_SIZE) as usize,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");

            for item in items {
                writer.write(item.as_item_ref()).await.expect("write failed");
            }

            writer.complete().await.expect("flush failed");
        }

        let layer = PersistentLayer::<ObjectKey, u64>::open(handle).await.expect("new failed");
        for target in to_find {
            let iterator = layer.seek(Bound::Included(&target)).await.expect("failed to seek");
            let ItemRef { key, .. } = iterator.get().expect("missing item");
            assert_eq!(&target, key);
        }
    }

    #[fuchsia::test]
    async fn test_two_seek_blocks() {
        // At the upper end of the supported size.
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const ITEMS_PER_PHASE: u64 = 50;
        const ITEM_COUNT: u64 = ITEMS_PER_PHASE * ((BLOCK_SIZE.size() / 8) + 2);

        let mut to_find = Vec::new();

        let handle = FakeObjectHandle::new_with_block_size(Arc::new(FakeObject::new()), BLOCK_SIZE);
        {
            let mut writer = PersistentLayerWriter::<_, TestKey, u64>::new(
                Writer::new(&handle).await,
                ITEM_COUNT as usize * 18,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");

            // Make all values take up maximum space for varint encoding.
            let initial_value = u32::MAX as u64 + 1;
            for i in 0..ITEM_COUNT {
                writer
                    .write(
                        Item::new(TestKey(initial_value + i..initial_value + i), initial_value)
                            .as_item_ref(),
                    )
                    .await
                    .expect("write failed");
            }
            // Look at the start middle and end.
            to_find.push(TestKey(initial_value..initial_value));
            let middle = initial_value + ITEM_COUNT / 2;
            to_find.push(TestKey(middle..middle));
            let end = initial_value + ITEM_COUNT - 1;
            to_find.push(TestKey(end..end));

            writer.complete().await.expect("flush failed");
        }

        let layer = PersistentLayer::<TestKey, u64>::open(handle).await.expect("new failed");
        for target in to_find {
            let iterator = layer.seek(Bound::Included(&target)).await.expect("failed to seek");
            let ItemRef { key, .. } = iterator.get().expect("missing item");
            assert_eq!(&target, key);
        }
    }

    // Verifies behaviour around creating full seek blocks, to ensure that it is able to be opened
    // and parsed afterward.
    #[fuchsia::test]
    async fn test_full_seek_block() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const ITEMS_PER_PHASE: u64 = 50;

        // How many entries there are in a seek table block.
        const SEEK_TABLE_ENTRIES: u64 = BLOCK_SIZE.size() / 8;

        // Number of entries to fill a seek block would need one more block of entries, but we're
        // starting low here on purpose to do a range and make sure we hit the size we are
        // interested in.
        const START_ENTRIES_COUNT: u64 = ITEMS_PER_PHASE * SEEK_TABLE_ENTRIES;

        for entries in START_ENTRIES_COUNT..START_ENTRIES_COUNT + (ITEMS_PER_PHASE * 2) {
            let handle =
                FakeObjectHandle::new_with_block_size(Arc::new(FakeObject::new()), BLOCK_SIZE);
            {
                let mut writer = PersistentLayerWriter::<_, TestKey, u64>::new(
                    Writer::new(&handle).await,
                    entries as usize,
                    BLOCK_SIZE,
                )
                .await
                .expect("writer new");

                // Make all values take up maximum space for varint encoding.
                let initial_value = u32::MAX as u64 + 1;
                for i in 0..entries {
                    writer
                        .write(
                            Item::new(TestKey(initial_value + i..initial_value + i), initial_value)
                                .as_item_ref(),
                        )
                        .await
                        .expect("write failed");
                }

                writer.complete().await.expect("flush failed");
            }
            PersistentLayer::<TestKey, u64>::open(handle).await.expect("new failed");
        }
    }

    #[fuchsia::test]
    async fn test_ignore_bloom_filter_on_older_versions() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        const ITEMS_PER_PHASE: u64 = 50;
        // Add enough items to create enough blocks for a bloom filter to be necessary.
        const ITEM_COUNT: u64 = (1 + MINIMUM_DATA_BLOCKS_FOR_BLOOM_FILTER as u64) * ITEMS_PER_PHASE;

        let old_version_handle =
            FakeObjectHandle::new_with_block_size(Arc::new(FakeObject::new()), BLOCK_SIZE);
        let current_version_handle =
            FakeObjectHandle::new_with_block_size(Arc::new(FakeObject::new()), BLOCK_SIZE);
        {
            let mut old_version_writer =
                PersistentLayerWriter::<_, TestKey, u64>::new_with_version(
                    Writer::new(&old_version_handle).await,
                    ITEM_COUNT as usize,
                    BLOCK_SIZE,
                    Version { major: LATEST_VERSION.major - 1, minor: 0 },
                )
                .await
                .expect("writer new");
            let mut current_version_writer = PersistentLayerWriter::<_, TestKey, u64>::new(
                Writer::new(&current_version_handle).await,
                ITEM_COUNT as usize,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");

            // Make all values take up maximum space for varint encoding.
            let initial_value = u32::MAX as u64 + 1;
            for i in 0..ITEM_COUNT {
                old_version_writer
                    .write(
                        Item::new(TestKey(initial_value + i..initial_value + i), initial_value)
                            .as_item_ref(),
                    )
                    .await
                    .expect("write failed");
                current_version_writer
                    .write(
                        Item::new(TestKey(initial_value + i..initial_value + i), initial_value)
                            .as_item_ref(),
                    )
                    .await
                    .expect("write failed");
            }

            old_version_writer.complete().await.expect("flush failed");
            current_version_writer.complete().await.expect("flush failed");
        }

        let old_layer =
            PersistentLayer::<TestKey, u64>::open(old_version_handle).await.expect("open failed");
        let current_layer = PersistentLayer::<TestKey, u64>::open(current_version_handle)
            .await
            .expect("open failed");
        assert!(!old_layer.has_bloom_filter());
        assert!(current_layer.has_bloom_filter());
    }

    #[fuchsia::test]
    async fn test_key_exists_no_bloom_filter() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_8KIB;
        // Not enough items to trigger a bloom filter.
        const ITEM_COUNT: i32 = 100;

        let handle = FakeObjectHandle::new_with_block_size(Arc::new(FakeObject::new()), BLOCK_SIZE);
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                ITEM_COUNT as usize,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            for i in 0..ITEM_COUNT {
                writer.write(Item::new(i * 2, i * 2).as_item_ref()).await.expect("write failed");
            }
            writer.complete().await.expect("flush failed");
        }
        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");
        assert!(!layer.has_bloom_filter());

        for i in 0..ITEM_COUNT {
            assert_eq!(
                layer.key_exists(&(i * 2)).await.expect("key_exists failed"),
                Existence::Exists
            );
            assert_eq!(
                layer.key_exists(&(i * 2 + 1)).await.expect("key_exists failed"),
                Existence::Missing
            );
        }
    }

    #[fuchsia::test]
    async fn test_key_exists_with_bloom_filter() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        // Enough items to trigger a bloom filter.
        const ITEM_COUNT: i32 = 10000;

        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                ITEM_COUNT as usize,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            for i in 0..ITEM_COUNT {
                writer.write(Item::new(i * 2, i * 2).as_item_ref()).await.expect("write failed");
            }
            writer.complete().await.expect("flush failed");
        }
        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("new failed");
        assert!(layer.has_bloom_filter());

        for i in 0..ITEM_COUNT {
            // With a bloom filter, we expect MaybeExists for present keys.
            assert_eq!(
                layer.key_exists(&(i * 2)).await.expect("key_exists failed"),
                Existence::MaybeExists
            );
        }

        // For missing keys, we expect Missing, but might get MaybeExists due to false positives.
        // We can at least assert it's NOT Exists.
        let mut missing_count = 0;
        for i in 0..ITEM_COUNT {
            let result = layer.key_exists(&(i * 2 + 1)).await.expect("key_exists failed");
            assert_ne!(result, Existence::Exists);
            if result == Existence::Missing {
                missing_count += 1;
            }
        }
        // We expect mostly Missing.
        assert!(missing_count > ITEM_COUNT / 2);
    }

    #[fuchsia::test]
    async fn test_load_large_bloom_filter_multi_chunk() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        // Sizing for 600_000 items creates a 2 MiB bloom filter (exceeding 1 MiB chunk size).
        const ESTIMATED_ITEMS: usize = 600_000;
        const WRITTEN_ITEMS: i32 = 2000;

        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                ESTIMATED_ITEMS,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            for i in 0..WRITTEN_ITEMS {
                writer.write(Item::new(i * 2, i * 2).as_item_ref()).await.expect("write failed");
            }
            writer.complete().await.expect("flush failed");
        }
        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("open failed");
        assert!(layer.has_bloom_filter());

        for i in 0..WRITTEN_ITEMS {
            assert_eq!(layer.maybe_contains_key(&(i * 2)), MaybeContainsKey::Maybe);
        }
        let mut false_count = 0;
        for i in 0..WRITTEN_ITEMS {
            if layer.maybe_contains_key(&(i * 2 + 1)) == MaybeContainsKey::False {
                false_count += 1;
            }
        }
        assert!(false_count > WRITTEN_ITEMS / 2);
    }

    #[fuchsia::test]
    async fn test_clear_cached_data() {
        const BLOCK_SIZE: BlockSize = BlockSize::SIZE_512B;
        let handle = FakeObjectHandle::new(Arc::new(FakeObject::new()));
        {
            let mut writer = PersistentLayerWriter::<_, i32, i32>::new(
                Writer::new(&handle).await,
                100,
                BLOCK_SIZE,
            )
            .await
            .expect("writer new");
            writer.write(Item::new(1, 1).as_item_ref()).await.expect("write failed");
            writer.complete().await.expect("flush failed");
        }
        let layer = PersistentLayer::<i32, i32>::open(handle).await.expect("open failed");
        let iter = layer.seek(Bound::Unbounded).await.expect("seek failed");
        assert_eq!(iter.get().map(|i| (*i.key, *i.value)), Some((1, 1)));
        drop(iter);

        assert!(layer.caching_object_handle.try_read(BLOCK_SIZE.get() as usize).is_some());

        layer.clear_cached_data();

        assert!(layer.caching_object_handle.try_read(BLOCK_SIZE.get() as usize).is_none());
    }
}
