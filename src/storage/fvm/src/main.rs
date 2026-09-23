// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod buffers;
mod device;
mod mapping;
mod zxcrypt;

use crate::zxcrypt::Key;
use anyhow::{Context, Error, anyhow, bail, ensure};
use block_client::{
    BlockClient, BufferSlice, MutableBufferSlice, ReadOptions, RemoteBlockClient, VmoId,
    WriteFlags, WriteOptions,
};
use block_server::async_interface::{Interface, SessionManager};
use block_server::{BlockServer, DeviceInfo, VolumeInfo};
use device::Device;
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker, ServerEnd};
use fidl_fuchsia_fs::{AdminMarker, AdminRequest, AdminRequestStream};
use fidl_fuchsia_fs_startup::{
    CheckOptions, CreateOptions, FormatOptions, MountOptions, StartOptions, StartupMarker,
    StartupRequest, StartupRequestStream, VolumeRequest, VolumeRequestStream, VolumesMarker,
    VolumesRequest, VolumesRequestStream,
};
use fidl_fuchsia_fvm::{ResetMarker, ResetRequest, ResetRequestStream};
use fidl_fuchsia_fxfs::CryptMarker;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block as fblock;
use fidl_fuchsia_storage_block::BlockMarker;
use fs_management::filesystem::{BlockConnector, Filesystem};
use fs_management::format::{DiskFormat, detect_disk_format};
use fs_management::{ComponentType, FSConfig, Options};
use fuchsia_async as fasync;
use fuchsia_inspect::Property as _;
use fuchsia_runtime::HandleType;
use fuchsia_sync::Mutex;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, TryStreamExt as _};
use log::{debug, error, info, warn};
use mapping::{Mapping, MappingExt as _};
use sha2::{Digest, Sha256};
use static_assertions::const_assert;
use std::alloc;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Formatter;
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZero;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;
use vfs::directory::helper::DirectlyMutable;
use vfs::execution_scope::ExecutionScope;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

// See //src/storage/fvm/format.h for a detailed description of the FVM format.

static MAGIC: u64 = 0x54524150204d5646;
static MAJOR_VERSION: u64 = 1;
static MINOR_VERSION: u64 = 1;

// Whilst this is the block size that FVM uses to round up certain structures, FVM still supports
// I/O at a smaller size than this (it will pass along the block size from the underlying device).
const BLOCK_SIZE: u64 = 8192;

// This is the maximum slice count which means the maximum slice offset is one less than this. Even
// though the format seems like it would support more than this, the legacy driver had this as the
// max so we will continue to as well.
const MAX_SLICE_COUNT: u64 = 1u64 << 31;

const MAX_PARTITIONS: u64 = 1024;

const DEFAULT_SLICE_SIZE: u64 = 32768;

/// A raw unverified struct for the Header.
#[repr(C)]
#[derive(Clone, Copy, Debug, KnownLayout, FromBytes, IntoBytes, Immutable)]
struct HeaderRaw {
    magic: u64,
    major_version: u64,
    pslice_count: u64,
    slice_size: u64,
    fvm_partition_size: u64,
    vpartition_table_size: u64,
    allocation_table_size: u64,
    generation: u64,
    hash: [u8; 32],
    oldest_minor_version: u64,
}

impl HeaderRaw {
    /// Returns the offset of the second copy of the metadata.
    fn offset_for_slot(&self, slot: u8) -> Result<u64, Error> {
        match slot {
            0 => Ok(0),
            1 => BLOCK_SIZE
                .checked_add(self.vpartition_table_size)
                .and_then(|n| n.checked_add(self.allocation_table_size))
                .ok_or_else(|| anyhow!("Overflow in offset_for_slot")),
            _ => unreachable!(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, KnownLayout, FromBytes, IntoBytes, Immutable)]
struct PartitionEntry {
    type_guid: [u8; 16],
    guid: [u8; 16],
    slices: u32,
    flags: u32,
    name: [u8; 24],
}

impl std::fmt::Debug for PartitionEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("PartitionEntry")
            .field("type_guid", &Uuid::from_slice(&self.type_guid).unwrap())
            .field("guid", &Uuid::from_slice(&self.guid).unwrap())
            .field("slices", &self.slices)
            .field("flags", &self.flags)
            .field("name", &self.name())
            .finish()
    }
}

impl PartitionEntry {
    fn is_allocated(&self) -> bool {
        self.slices > 0
    }

    fn name(&self) -> Cow<'_, str> {
        // Find the first NULL character.
        let end = self.name.iter().position(|c| *c == 0).unwrap_or(24);
        // TODO(https://fxbug.dev/357467643): Make sure names are unique and not empty.
        match std::str::from_utf8(&self.name[..end]) {
            Ok(name) => Cow::Borrowed(name),
            Err(_) => Cow::Owned(format!("{}", Uuid::from_slice(&self.guid).unwrap())),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, KnownLayout, FromBytes, IntoBytes, Immutable)]
struct SliceEntry(u64);

impl SliceEntry {
    fn partition_index(&self) -> u16 {
        self.0 as u16
    }

    fn logical_slice(&self) -> u64 {
        self.0 >> 16
    }

    fn set(&mut self, partition_index: u16, logical_slice: u64) {
        self.0 = partition_index as u64 | logical_slice << 16;
    }
}

struct Fvm {
    device: Arc<Device>,

    // The slize size in blocks.
    slice_blocks: u64,

    // We use an async lock to make it easier to mediate safe access to the metadata.  When we
    // mutate the metadata, we need to hold a lock whilst writing the metadata (which is done using
    // async code) so that no other mutations can race.  When performing regular I/O, we can iterate
    // through mappings and perform async I/O without having to drop the lock.
    inner: async_lock::RwLock<Inner>,

    _fvm_inspect: fuchsia_inspect::Node,
    per_partition_inspect: fuchsia_inspect::Node,
}

struct Inner {
    slot: u8,
    metadata: Metadata,
    partition_state: HashMap<u16, PartitionState>,
    assigned_slice_count: u64,
}

impl Inner {
    async fn write_new_metadata_to(
        &mut self,
        device: &RemoteBlockClient,
        mut new_metadata: Metadata,
    ) -> Result<(), Error> {
        new_metadata.header.generation = new_metadata
            .header
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow!(zx::Status::BAD_STATE))?;

        let new_slot = 1 - self.slot;
        let offset = new_metadata.offset_for_slot(new_slot);
        new_metadata.write(device, offset).await?;

        self.slot = new_slot;
        self.metadata = new_metadata;

        Ok(())
    }
}

struct PartitionInspect {
    _root: fuchsia_inspect::Node,
    total_slices_reserved: fuchsia_inspect::UintProperty,
    max_bytes: fuchsia_inspect::UintProperty,
}

impl PartitionInspect {
    fn new<'a>(
        per_partition_inspect: &fuchsia_inspect::Node,
        name: impl Into<Cow<'a, str>>,
        num_slices: u64,
    ) -> Self {
        let root = per_partition_inspect.create_child(name);
        let total_slices_reserved = root.create_uint("total_slices_reserved", num_slices);
        let max_bytes = root.create_uint("max_bytes", 0);
        PartitionInspect { _root: root, total_slices_reserved, max_bytes }
    }

    fn update_total_slices(&self, num_slices: u64) {
        self.total_slices_reserved.set(num_slices);
    }

    fn update_max_bytes(&self, max_bytes: u64) {
        self.max_bytes.set(max_bytes);
    }
}

struct PartitionState {
    mappings: Vec<Mapping>,
    slice_limit: u64,
    partition_inspect: PartitionInspect,
}

#[derive(Clone)]
struct Metadata {
    /// The header is only placed inside this struct once the values inside have been verified to
    /// be sensible. The `hash` field of the header is not necessarily up to date and must be
    /// recomputed before writing.
    header: HeaderRaw,

    partitions: BTreeMap<u16, PartitionEntry>,
    allocations: Vec<SliceEntry>,
}

impl Metadata {
    async fn read(
        header_block: &[u8],
        device: &RemoteBlockClient,
        offset: u64,
    ) -> Result<Self, Error> {
        let (header, _) = HeaderRaw::ref_from_prefix(header_block)
            .map_err(|_| anyhow!("Block size too small"))?;
        ensure!(header.magic == MAGIC, "Magic mismatch");
        ensure!(
            header.slice_size > 0 && header.slice_size % BLOCK_SIZE == 0,
            format!("Slice size ({}) not a non-zero multiple of {BLOCK_SIZE}", header.slice_size)
        );

        // Read the vpartition and allocation table.
        let expected_partition_table_size =
            MAX_PARTITIONS * std::mem::size_of::<PartitionEntry>() as u64;
        ensure!(
            header.vpartition_table_size == expected_partition_table_size,
            "Bad vpartition table size"
        );

        let max_allocation_table_size = MAX_SLICE_COUNT * std::mem::size_of::<SliceEntry>() as u64;
        ensure!(
            header.allocation_table_size > 0
                && header.allocation_table_size <= max_allocation_table_size
                && header.allocation_table_size % BLOCK_SIZE == 0,
            "Bad allocation table size"
        );

        ensure!(header.pslice_count <= MAX_SLICE_COUNT, "pslice_count exceeds maximum");

        let allocation_size = (header.pslice_count + 1)
            .checked_mul(std::mem::size_of::<SliceEntry>() as u64)
            .and_then(|n| n.checked_next_multiple_of(BLOCK_SIZE))
            .ok_or_else(|| anyhow!("Bad pslice_count"))
            .map(|n| n as usize)?;
        ensure!(
            header.allocation_table_size >= allocation_size as u64,
            "Allocation table size too small"
        );

        // Verify that all the calculations required will stay in-bounds.
        let data_size_needed = header
            .pslice_count
            .checked_mul(header.slice_size)
            .ok_or_else(|| anyhow!("Calculated data area size too large"))?;
        header
            .vpartition_table_size
            .checked_add(header.allocation_table_size)
            .and_then(|n| n.checked_add(BLOCK_SIZE))
            .and_then(|n| n.checked_mul(2))
            .and_then(|n| n.checked_add(data_size_needed))
            .ok_or_else(|| anyhow!("Required device size overflowed"))?;

        let part_table_size = header.vpartition_table_size as usize;
        let mut buffer = vec![0; part_table_size + allocation_size];
        device.read_at(MutableBufferSlice::Memory(&mut buffer), offset + BLOCK_SIZE).await?;

        // Check the hash.
        let mut hasher = Sha256::new();
        let mut header_copy = *header;
        header_copy.hash = [0; 32];
        hasher.update(header_copy.as_bytes());
        hasher.update(&header_block[std::mem::size_of::<HeaderRaw>()..]);
        hasher.update(&buffer);

        if hasher.finalize().as_slice() != header.hash {
            return Err(zx::Status::IO_DATA_INTEGRITY).context("Hash mismatch");
        }

        let partitions = buffer[..part_table_size]
            .chunks_exact(std::mem::size_of::<PartitionEntry>())
            .enumerate()
            .skip(1) // The first partition is unused.
            .filter_map(|(index, e)| {
                let partition = PartitionEntry::read_from_bytes(e).unwrap();
                partition.is_allocated().then_some((index as u16, partition))
            })
            .collect();
        let allocations = buffer[part_table_size..part_table_size + allocation_size]
            .chunks_exact(std::mem::size_of::<SliceEntry>())
            .skip(1) // The first slice entry is unused.
            .take(header.pslice_count as usize)
            .map(|e| SliceEntry::read_from_bytes(e).unwrap())
            .collect();

        Ok(Self { header: header_copy, partitions, allocations })
    }

    fn used_allocation_table_size(&self) -> usize {
        // These values are validated at construction time.
        ((self.header.pslice_count + 1) * std::mem::size_of::<SliceEntry>() as u64)
            .next_multiple_of(BLOCK_SIZE) as usize
    }

    /// Returns the offset where the data starts.
    fn data_start(&self) -> u64 {
        // These value validated at construction time.
        (BLOCK_SIZE + self.header.vpartition_table_size + self.header.allocation_table_size) * 2
    }

    fn max_allocation_table_entries(&self) -> u64 {
        // The 0 indexed slice in the allocation table is unused, but is included in the table size
        // because it is still written to disk as an empty slice entry. When calculating the number
        // of available table entries we don't count it, hence subtracting 1.
        (self.header.allocation_table_size / std::mem::size_of::<SliceEntry>() as u64)
            .saturating_sub(1)
    }

    fn offset_for_slot(&self, slot: u8) -> u64 {
        // Unwrap because the values used should already be validated.
        self.header.offset_for_slot(slot).unwrap()
    }

    async fn write(&self, device: &RemoteBlockClient, offset: u64) -> Result<(), Error> {
        // We align the memory to that required by `Header`, but the memory needs to be aligned for
        // `PartitionEntry` and `SliceEntry` too.
        const_assert!(std::mem::align_of::<HeaderRaw>() >= std::mem::align_of::<PartitionEntry>());
        const_assert!(std::mem::align_of::<HeaderRaw>() >= std::mem::align_of::<SliceEntry>());

        let mut buffer = AlignedMem::<HeaderRaw>::new(
            (BLOCK_SIZE + self.header.vpartition_table_size) as usize
                + self.used_allocation_table_size(),
        );
        let (header, _) = HeaderRaw::mut_from_prefix(&mut buffer).unwrap();
        *header = self.header;
        header.hash.fill(0);

        // Write out the partitions:
        for (&index, partition) in &self.partitions {
            let (entry, _) = PartitionEntry::mut_from_prefix(
                &mut buffer[BLOCK_SIZE as usize
                    + std::mem::size_of::<PartitionEntry>() * index as usize..],
            )
            .unwrap();
            *entry = *partition;
        }

        // Write out the allocation table.  The first slice entry is unused.
        <[SliceEntry]>::mut_from_bytes(
            &mut buffer[(BLOCK_SIZE + self.header.vpartition_table_size) as usize..],
        )
        .unwrap()[1..1 + self.allocations.len()]
            .copy_from_slice(&self.allocations);

        // Compute the hash.
        let mut hasher = Sha256::new();
        hasher.update(&*buffer);
        let (header, _) = HeaderRaw::mut_from_prefix(&mut buffer).unwrap();
        header.hash.copy_from_slice(hasher.finalize().as_slice());

        device.write_at(BufferSlice::Memory(&buffer), offset).await?;

        // Always flush after writing metadata.
        device.flush().await?;

        Ok(())
    }

    /// Allocates slices.  NOTE: This will leave the metadata in an inconsistent state if this
    /// fails.
    fn allocate_slices(
        &mut self,
        partition_index: u16,
        mut logical_slice: u64,
        mut count: u64,
    ) -> Result<Vec<Mapping>, zx::Status> {
        let max_slice = self.header.pslice_count;
        let partition = self.partitions.get_mut(&partition_index).unwrap();
        partition.slices = partition
            .slices
            .checked_add(count.try_into().map_err(|_| zx::Status::NO_SPACE)?)
            .ok_or(zx::Status::NO_SPACE)?;
        let mut mappings = Vec::<Mapping>::new();
        for (physical_slice, allocation) in self.allocations.iter_mut().enumerate() {
            if physical_slice as u64 == max_slice {
                break;
            }

            if allocation.partition_index() == 0 {
                allocation.set(partition_index, logical_slice);
                let add_new_mapping = match mappings.last_mut() {
                    Some(mapping) if mapping.to + mapping.len() == physical_slice as u64 => {
                        mapping.from.end += 1;
                        false
                    }
                    _ => true,
                };
                if add_new_mapping {
                    mappings.push(Mapping {
                        from: logical_slice..logical_slice + 1,
                        to: physical_slice as u64,
                    });
                }
                logical_slice += 1;
                count -= 1;
                if count == 0 {
                    return Ok(mappings);
                }
            }
        }
        Err(zx::Status::NO_SPACE)
    }

    fn set_pslice_count(&mut self, pslice_count: u64) -> Result<(), Error> {
        ensure!(pslice_count <= MAX_SLICE_COUNT, "pslice_count exceeds maximum");
        self.header.pslice_count = pslice_count;
        let data_start = self.data_start();
        let slice_data_size = pslice_count
            .checked_mul(self.header.slice_size)
            .ok_or_else(|| anyhow!("Overflow in slice data size"))?;
        self.header.fvm_partition_size = data_start
            .checked_add(slice_data_size)
            .ok_or_else(|| anyhow!("Overflow in fvm_partition_size"))?;
        self.allocations.resize(pslice_count as usize, SliceEntry(0));
        Ok(())
    }
}

impl Fvm {
    /// Opens the FVM device.
    pub async fn open(client: RemoteBlockClient) -> Result<Self, Error> {
        ensure!(BLOCK_SIZE as u32 % client.block_size() == 0, zx::Status::NOT_SUPPORTED);

        let mut inner = {
            let mut header_block = AlignedMem::<HeaderRaw>::new(BLOCK_SIZE as usize);
            client.read_at(MutableBufferSlice::Memory(&mut header_block), 0).await?;

            let metadata_a = Metadata::read(&header_block, &client, 0).await;

            let (header, _) = HeaderRaw::ref_from_prefix(&header_block)
                .map_err(|_| anyhow!("Block size too small"))?;
            let secondary_offset = header.offset_for_slot(1)?;
            client.read_at(MutableBufferSlice::Memory(&mut header_block), secondary_offset).await?;

            let metadata_b = Metadata::read(&header_block, &client, secondary_offset).await;

            let (slot, metadata) =
                Self::pick_metadata(metadata_a, metadata_b).ok_or_else(|| {
                    warn!("No valid metadata");
                    anyhow!("No valid metadata")
                })?;
            Inner {
                slot: slot as u8,
                metadata,
                partition_state: HashMap::new(),
                assigned_slice_count: 0,
            }
        };

        // It's possible the disk size is larger than we knew. This might happen if the original
        // preallocated metadata had some size set when the image was created but the device it is
        // flashed to has a larger disk than that. If we notice this happen, we need to expand the
        // number of physical slices available up to whatever maximum the metadata allows for.
        let max_data_slices;
        {
            let header = &inner.metadata.header;
            let device_size = client.block_count() * client.block_size() as u64;
            let start_offset = inner.metadata.data_start();
            max_data_slices = std::cmp::min(
                device_size
                    .checked_sub(start_offset)
                    .ok_or_else(|| anyhow!("Disk is too small for an FVM volume"))?
                    / header.slice_size,
                inner.metadata.max_allocation_table_entries(),
            );
            if max_data_slices > header.pslice_count {
                // We need to update the pslice count in the header and then use that going forward.
                info!("Growing allocation table {} -> {max_data_slices}", header.pslice_count);
                let mut new_metadata = inner.metadata.clone();
                new_metadata.set_pslice_count(max_data_slices)?;
                inner.write_new_metadata_to(&client, new_metadata).await?;
            }
        }

        let fvm_inspect = fuchsia_inspect::component::inspector().root().create_child("fvm");
        let per_partition_inspect = fvm_inspect.create_child("partitions");

        // Build the mappings.
        let metadata = &mut inner.metadata;
        for (idx, partition) in metadata.partitions.iter() {
            inner.partition_state.insert(
                *idx,
                PartitionState {
                    mappings: Vec::new(),
                    slice_limit: 0,
                    partition_inspect: PartitionInspect::new(
                        &per_partition_inspect,
                        partition.name(),
                        partition.slices as u64,
                    ),
                },
            );
        }
        for (physical_slice, allocation) in metadata.allocations.iter().enumerate() {
            let partition_index = allocation.partition_index();
            let slice = allocation.logical_slice();
            if partition_index == 0 {
                // Entry is free.
                continue;
            }
            if !metadata.partitions.contains_key(&partition_index) {
                warn!("Slice entry points to free partition: 0x{:x?}", allocation.0);
                continue;
            }
            if physical_slice as u64 >= max_data_slices {
                return Err(zx::Status::IO_DATA_INTEGRITY).context(
                    "Volume has allocated slice {physical_slice} beyond end of disk \
                              ({max_data_slices})",
                );
            }
            inner.assigned_slice_count += 1;
            // unwrap safety: map is made from metadata.partitions right before this and we make
            // sure the key exists in that map.
            let mappings = &mut inner.partition_state.get_mut(&partition_index).unwrap().mappings;
            let mut bad_mapping = false;
            match mappings.binary_search_by(|m: &Mapping| m.from.start.cmp(&slice)) {
                Ok(_) => bad_mapping = true,
                Err(index) => {
                    let insert = if index > 0 {
                        // See if this can be merged with the previous entry.
                        let prev_mapping = &mut mappings[index - 1];
                        let end = prev_mapping.from.end;
                        if end == slice
                            && prev_mapping.to + prev_mapping.len() == physical_slice as u64
                        {
                            prev_mapping.from.end += 1;
                            false
                        } else if end <= slice {
                            true
                        } else {
                            bad_mapping = true;
                            false
                        }
                    } else {
                        true
                    };
                    if insert {
                        mappings.insert(
                            index,
                            Mapping { from: slice..slice + 1, to: physical_slice as u64 },
                        );
                    }
                }
            };
            if bad_mapping {
                warn!("Duplicate slice entry: 0x{:x?}", allocation.0);
            }
        }

        if metadata.header.pslice_count > max_data_slices {
            warn!("Number of slices exceeds disk bounds (continuing since none are allocated)");
            // There aren't any allocated slices (we check that earlier), but we need to prevent the
            // slices from being allocated, so pslice_count is updated here.
            metadata.set_pslice_count(max_data_slices)?;
        }

        info!(
            "Mounted fvm, block size {}, slice size {} ({}/{} allocated) partitions: {:?}",
            client.block_size(),
            metadata.header.slice_size,
            inner.assigned_slice_count,
            metadata.header.pslice_count,
            metadata.partitions.iter().map(|(_, e)| e.name()).collect::<Vec<_>>()
        );
        for (idx, partition) in metadata.partitions.iter() {
            info!("  {idx}: {partition:?}");
        }

        // Record the mount-time inspect metrics
        fvm_inspect.record_child("mount_time", |mount_time| {
            mount_time.record_uint("major_version", metadata.header.major_version);
            mount_time.record_uint("oldest_minor_version", metadata.header.oldest_minor_version);
            mount_time.record_string(
                "version_combo",
                format!(
                    "{}/{}",
                    metadata.header.major_version, metadata.header.oldest_minor_version
                ),
            );
            mount_time.record_uint("slice_size", metadata.header.slice_size);
            mount_time.record_uint("num_slices", metadata.header.pslice_count);
            mount_time.record_uint("partition_table_entries", MAX_PARTITIONS - 1);
            mount_time.record_uint("partition_table_reserved_entries", MAX_PARTITIONS - 1);
            mount_time.record_uint("allocation_table_entries", metadata.header.pslice_count);
            mount_time.record_uint(
                "allocation_table_reserved_entries",
                metadata.max_allocation_table_entries(),
            );
            mount_time.record_uint("num_partitions", metadata.partitions.len() as u64);
            mount_time.record_uint("num_reserved_slices", 0);
        });

        let slice_blocks = metadata.header.slice_size / client.block_size() as u64;
        Ok(Self {
            device: Arc::new(Device::new(client).await?),
            slice_blocks,
            inner: async_lock::RwLock::new(inner),
            _fvm_inspect: fvm_inspect,
            per_partition_inspect,
        })
    }

    fn block_size(&self) -> u32 {
        self.device.block_size()
    }

    fn pick_metadata(
        metadata_a: Result<Metadata, Error>,
        metadata_b: Result<Metadata, Error>,
    ) -> Option<(usize, Metadata)> {
        match (metadata_a, metadata_b) {
            (Ok(metadata_a), Ok(metadata_b)) => {
                if metadata_a.header.generation >= metadata_b.header.generation {
                    Some((0, metadata_a))
                } else {
                    Some((1, metadata_b))
                }
            }
            (Ok(metadata_a), Err(error)) => {
                warn!(error:?; "Bad metadata in slot 1, using slot 0");
                Some((0, metadata_a))
            }
            (Err(error), Ok(metadata_b)) => {
                warn!(error:?; "Bad metadata in slot 0, using slot 1");
                Some((1, metadata_b))
            }
            (Err(error_a), Err(error_b)) => {
                warn!(error_a:?; "Bad metadata in slot 0");
                warn!(error_b:?; "Bad metadata in slot 1");
                None
            }
        }
    }

    async fn do_io(
        &self,
        mut io: impl IoTrait,
        partition_index: u16,
        device_block_offset: u64,
        block_count: u32,
        trace_flow_id: u64,
    ) -> Result<(), Error> {
        let inner = self.inner.read().await;
        let Some(PartitionState { mappings, .. }) = inner.partition_state.get(&partition_index)
        else {
            bail!(zx::Status::INTERNAL);
        };

        let block_size = self.block_size() as u64;
        let mut offset = device_block_offset
            .checked_mul(block_size)
            .ok_or(zx::Status::OUT_OF_RANGE)
            .with_context(|| format!("Bad offset ({device_block_offset})"))?;
        let mut total_len = (block_count as u64)
            .checked_mul(block_size)
            .ok_or(zx::Status::OUT_OF_RANGE)
            .with_context(|| format!("Bad count ({block_count})"))?;

        let metadata = &inner.metadata;
        let slice_size = metadata.header.slice_size;
        let data_start = metadata.data_start();

        while total_len > 0 {
            let slice = offset / slice_size;
            let index = match mappings.binary_search_by(|m| m.from.start.cmp(&slice)) {
                Ok(index) => index,
                Err(index) if index > 0 => index - 1,
                _ => {
                    return Err(zx::Status::OUT_OF_RANGE).with_context(|| {
                        format!("No mapping #1 ({device_block_offset}, {block_count})")
                    });
                }
            };
            let mapping = &mappings[index];
            if slice >= mapping.from.end {
                return Err(zx::Status::OUT_OF_RANGE).with_context(|| {
                    format!("No mapping #2 ({device_block_offset}, {block_count})")
                });
            }
            let end = mapping.from.end * slice_size;
            let len = std::cmp::min(end - offset, total_len);
            let physical_offset =
                data_start + mapping.to * slice_size + (offset - mapping.from.start * slice_size);

            io.add_op(physical_offset, len, trace_flow_id).await?;
            offset += len;
            total_len -= len;
        }
        io.flush().await?;
        Ok(())
    }

    async fn create_partition(
        &self,
        inner: async_lock::RwLockUpgradableReadGuard<'_, Inner>,
        type_guid: [u8; 16],
        guid: [u8; 16],
        slices: u32,
        name_str: &str,
    ) -> Result<u16, Error> {
        ensure!(slices > 0, zx::Status::INVALID_ARGS);
        let name_len = name_str.as_bytes().len();
        ensure!(name_len <= 24, zx::Status::INVALID_ARGS);

        // Find a free partition
        let mut proposed = 1;
        for (&index, _) in &inner.metadata.partitions {
            if proposed != index {
                break;
            }
            let Some(next) = index.checked_add(1) else {
                bail!(zx::Status::NO_SPACE);
            };
            proposed = next;
        }

        let max_partitions = std::cmp::min(
            inner.metadata.header.vpartition_table_size
                / std::mem::size_of::<PartitionEntry>() as u64,
            MAX_PARTITIONS,
        );
        ensure!((proposed as u64) < max_partitions, zx::Status::NO_SPACE);

        let mut new_metadata = inner.metadata.clone();

        // Allocate slices:
        let mut name = [0; 24];
        name[..name_len].copy_from_slice(name_str.as_bytes());
        new_metadata
            .partitions
            .insert(proposed, PartitionEntry { type_guid, guid, slices: 0, flags: 0, name });

        let slices = slices as u64;
        let mappings = new_metadata.allocate_slices(proposed, 0, slices)?;

        let mut inner = self
            .write_new_metadata(inner, new_metadata)
            .await
            .context("Failed to write metadata")?;

        inner.assigned_slice_count = inner.assigned_slice_count.checked_add(slices).unwrap();
        let partition_inspect =
            PartitionInspect::new(&self.per_partition_inspect, name_str, slices as u64);
        inner
            .partition_state
            .insert(proposed, PartitionState { mappings, slice_limit: 0, partition_inspect });

        Ok(proposed)
    }

    async fn write_new_metadata<'a>(
        &self,
        inner: async_lock::RwLockUpgradableReadGuard<'a, Inner>,
        new_metadata: Metadata,
    ) -> Result<async_lock::RwLockWriteGuard<'a, Inner>, Error> {
        let mut inner = async_lock::RwLockUpgradableReadGuard::upgrade(inner).await;
        inner.write_new_metadata_to(&self.device, new_metadata).await?;
        Ok(inner)
    }

    /// Ensures the first `slices` slices are allocated.
    async fn ensure_allocated(&self, partition_index: u16, slices: u64) -> Result<(), zx::Status> {
        let inner = self.inner.upgradable_read().await;

        let partition_state = &inner.partition_state[&partition_index];
        let mappings = &partition_state.mappings;

        let mut new_metadata = inner.metadata.clone();
        let mut new_mappings = Vec::new();
        let mut next_slice = 0;
        let mut allocated = 0;
        for mapping in mappings {
            if mapping.from.start >= slices {
                break;
            }
            let count = mapping.from.start - next_slice;
            if count > 0 {
                new_mappings.push(new_metadata.allocate_slices(
                    partition_index,
                    next_slice,
                    count,
                )?);
                allocated += count;
            }
            next_slice = mapping.from.end;
        }
        if next_slice < slices {
            let count = slices - next_slice;
            new_mappings.push(new_metadata.allocate_slices(partition_index, next_slice, count)?);
            allocated += count;
        }

        if partition_state.slice_limit > 0
            && (inner.metadata.partitions[&partition_index].slices as u64)
                .checked_add(allocated)
                .map_or(true, |s| s > partition_state.slice_limit)
        {
            return Err(zx::Status::NO_SPACE);
        }

        let mut inner =
            self.write_new_metadata(inner, new_metadata).await.map_err(map_to_status)?;

        inner.assigned_slice_count = inner.assigned_slice_count.checked_add(allocated).unwrap();

        for m in new_mappings {
            inner
                .partition_state
                .get_mut(&partition_index)
                .unwrap()
                .mappings
                .insert_contiguous_mappings(m);
        }

        Ok(())
    }

    async fn get_info(&self) -> fblock::VolumeManagerInfo {
        let inner = self.inner.read().await;
        fblock::VolumeManagerInfo {
            slice_size: inner.metadata.header.slice_size,
            slice_count: inner.metadata.header.pslice_count,
            assigned_slice_count: inner.assigned_slice_count,
            maximum_slice_count: inner.metadata.max_allocation_table_entries(),
            max_virtual_slice: MAX_SLICE_COUNT,
        }
    }

    #[cfg(test)]
    async fn find_partition_with_name(&self, name: &str) -> Option<u16> {
        self.inner
            .read()
            .await
            .metadata
            .partitions
            .iter()
            .find_map(|(&index, p)| (p.name() == name).then_some(index))
    }
}

// Trait to abstract over the difference between reads and writes.
trait IoTrait {
    // Called to get the future that performs the read or write.
    fn add_op(
        &mut self,
        offset: u64,
        len: u64,
        trace_flow_id: u64,
    ) -> impl Future<Output = Result<(), zx::Status>>;

    fn flush(&mut self) -> impl Future<Output = Result<(), zx::Status>>;
}

fn trace_id(trace_flow_id: Option<NonZero<u64>>) -> u64 {
    trace_flow_id.map(|v| v.get()).unwrap_or_default()
}

struct Read<'a> {
    device: &'a Device,
    vmo_id: &'a VmoId,
    ops: FuturesUnordered<BoxFuture<'a, Result<(), zx::Status>>>,
    vmo_offset: u64,
}

impl IoTrait for Read<'_> {
    async fn add_op(
        &mut self,
        offset: u64,
        len: u64,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        self.ops.push(Box::pin(self.device.read_at_with_opts_traced(
            MutableBufferSlice::new_with_vmo_id(self.vmo_id, self.vmo_offset, len),
            offset,
            ReadOptions::default(),
            trace_flow_id,
        )));
        self.vmo_offset += len;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), zx::Status> {
        while self.ops.try_next().await?.is_some() {}
        Ok(())
    }
}

// Reads to memory.
struct ReadToMem<'a> {
    device: &'a Device,
    buffer: &'a mut [u8],
}

impl<'a> ReadToMem<'a> {
    fn new(device: &'a Device, buffer: &'a mut [u8]) -> Self {
        Self { device, buffer }
    }
}

impl IoTrait for ReadToMem<'_> {
    async fn add_op(
        &mut self,
        offset: u64,
        len: u64,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        // We don't care about performance, so we issue the operation immediately.
        let (head, tail) = std::mem::take(&mut self.buffer).split_at_mut(len as usize);
        self.buffer = tail;
        self.device
            .read_at_with_opts_traced(
                MutableBufferSlice::Memory(head),
                offset,
                ReadOptions::default(),
                trace_flow_id,
            )
            .await?;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), zx::Status> {
        Ok(())
    }
}

struct Write<'a> {
    device: &'a Device,
    vmo_id: &'a VmoId,
    ops: FuturesUnordered<BoxFuture<'a, Result<(), zx::Status>>>,
    vmo_offset: u64,
    options: WriteOptions,
}

impl IoTrait for Write<'_> {
    async fn add_op(
        &mut self,
        offset: u64,
        len: u64,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        self.ops.push(Box::pin(self.device.write_at_with_opts_traced(
            BufferSlice::new_with_vmo_id(self.vmo_id, self.vmo_offset, len),
            offset,
            self.options,
            trace_flow_id,
        )));
        self.vmo_offset += len;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), zx::Status> {
        while self.ops.try_next().await?.is_some() {}
        Ok(())
    }
}

// Writes from memory.
struct WriteFromMem<'a> {
    device: &'a Device,
    buffer: &'a [u8],
}

impl<'a> WriteFromMem<'a> {
    fn new(device: &'a Device, buffer: &'a [u8]) -> Self {
        Self { device, buffer }
    }
}

impl IoTrait for WriteFromMem<'_> {
    async fn add_op(
        &mut self,
        offset: u64,
        len: u64,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        // We don't care about performance, so we issue the operation immediately.
        let (head, tail) = self.buffer.split_at(len as usize);
        self.buffer = tail;
        self.device
            .write_at_with_opts_traced(
                BufferSlice::Memory(head),
                offset,
                WriteOptions::default(),
                trace_flow_id,
            )
            .await?;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), zx::Status> {
        Ok(())
    }
}

struct Trim<'a> {
    device: &'a Device,
    ops: FuturesUnordered<BoxFuture<'a, Result<(), zx::Status>>>,
}

impl IoTrait for Trim<'_> {
    async fn add_op(
        &mut self,
        offset: u64,
        len: u64,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        self.ops.push(Box::pin(self.device.trim_traced(offset..offset + len, trace_flow_id)));
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), zx::Status> {
        while self.ops.try_next().await?.is_some() {}
        Ok(())
    }
}

struct MountedVolumeInner {
    pub scope: ExecutionScope,
    pub block_server: BlockServer<SessionManager<PartitionInterface>>,
    pub shutdown: AtomicBool,
}

#[derive(Clone)]
struct MountedVolume(Arc<MountedVolumeInner>);

impl Deref for MountedVolume {
    type Target = Arc<MountedVolumeInner>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl MountedVolume {
    pub fn new(block_server: BlockServer<SessionManager<PartitionInterface>>) -> Self {
        Self(Arc::new(MountedVolumeInner {
            scope: ExecutionScope::new(),
            block_server,
            shutdown: AtomicBool::new(false),
        }))
    }
}

impl BlockConnector for MountedVolume {
    fn connect_channel_to_block(
        &self,
        server_end: ServerEnd<fblock::BlockMarker>,
    ) -> Result<(), Error> {
        let this = self.clone();
        self.scope.spawn(async move {
            let _ = this.block_server.handle_requests(server_end.into_stream()).await;
        });
        Ok(())
    }
}

/// Serves a multi-filesystem component that uses the FVM format.
struct Component {
    export_dir: Arc<vfs::directory::immutable::Simple>,
    scope: ExecutionScope,
    fvm: Mutex<Option<Arc<Fvm>>>,
    mounted: Mutex<HashMap<u16, MountedVolume>>,
    volumes_directory: Arc<vfs::directory::immutable::Simple>,
    inspect_server_task: Mutex<Option<inspect_runtime::PublishedInspectController>>,
}

impl Component {
    pub fn new() -> Self {
        Self {
            export_dir: vfs::directory::immutable::simple(),
            scope: ExecutionScope::new(),
            fvm: Mutex::default(),
            mounted: Mutex::default(),
            volumes_directory: vfs::directory::immutable::simple(),
            inspect_server_task: Mutex::default(),
        }
    }

    /// Serves an outgoing directory on `outgoing_dir`.
    pub async fn serve(self: &Arc<Self>, outgoing_dir: zx::Channel) -> Result<(), Error> {
        let svc_dir = vfs::directory::immutable::simple();
        self.export_dir.add_entry("svc", svc_dir.clone()).expect("Unable to create svc dir");

        *self.inspect_server_task.lock() = inspect_runtime::publish(
            fuchsia_inspect::component::inspector(),
            inspect_runtime::PublishOptions::default(),
        );

        let weak = Arc::downgrade(self);
        svc_dir.add_entry(
            StartupMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        let _ = me.handle_startup_requests(requests).await;
                    }
                }
            }),
        )?;
        let weak = Arc::downgrade(self);
        svc_dir.add_entry(
            VolumesMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        let _ = me.handle_volumes_requests(requests).await;
                    }
                }
            }),
        )?;
        let weak = Arc::downgrade(self);
        svc_dir.add_entry(
            AdminMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        let _ = me.handle_admin_requests(requests).await;
                    }
                }
            }),
        )?;
        let weak = Arc::downgrade(self);
        svc_dir.add_entry(
            ResetMarker::PROTOCOL_NAME,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        let _ = me.handle_reset_requests(requests).await;
                    }
                }
            }),
        )?;
        vfs::directory::serve_on(
            self.export_dir.clone(),
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
            self.scope.clone(),
            ServerEnd::new(outgoing_dir),
        );
        Ok(())
    }

    async fn handle_startup_requests(
        self: &Arc<Self>,
        mut stream: StartupRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                StartupRequest::Start { responder, device, options } => responder
                    .send(self.handle_start(device, options).await.map_err(map_to_raw_status))?,
                StartupRequest::Format { responder, device, options } => responder
                    .send(self.handle_format(device, options).await.map_err(map_to_raw_status))?,
                StartupRequest::Check { responder, .. } => {
                    responder.send(Err(zx::Status::NOT_SUPPORTED.into_raw()))?;
                }
            }
        }
        Ok(())
    }

    /// Handles the `Format` request from `fuchsia.fs.startup.Startup` protocol.
    /// This formats the provided block device as an empty FVM volume.
    async fn handle_format(
        self: &Arc<Self>,
        device: fidl::endpoints::ClientEnd<fblock::BlockMarker>,
        options: FormatOptions,
    ) -> Result<(), Error> {
        let block_proxy = device.into_proxy();
        let info = block_proxy.get_info().await?.map_err(zx::Status::err_from_raw)?;
        let disk_size = info.block_count * info.block_size as u64;

        let slice_size = options.fvm_slice_size.unwrap_or(DEFAULT_SLICE_SIZE);
        if slice_size < BLOCK_SIZE || !slice_size.is_power_of_two() {
            return Err(zx::Status::INVALID_ARGS).context("Bad slice size {slice_size}");
        }
        let vpartition_table_size = (MAX_PARTITIONS * std::mem::size_of::<PartitionEntry>() as u64)
            .next_multiple_of(BLOCK_SIZE);

        // This uses the same strategy as the C++ FVM code: it chooses the allocation table size
        // assuming the metadata space is zero.  This results in a table size that's bigger than it
        // might need to be.
        let allocation_table_size = (((disk_size / slice_size) + 1)
            * (std::mem::size_of::<SliceEntry>() as u64))
            .next_multiple_of(BLOCK_SIZE);

        let header = HeaderRaw {
            magic: MAGIC,
            major_version: MAJOR_VERSION,
            pslice_count: 0,
            slice_size,
            fvm_partition_size: disk_size,
            vpartition_table_size,
            allocation_table_size,
            generation: 0,
            hash: [0; 32],
            oldest_minor_version: MINOR_VERSION,
        };

        let mut metadata = Metadata {
            header,
            partitions: std::collections::BTreeMap::new(),
            allocations: vec![SliceEntry(0); header.pslice_count as usize],
        };

        // Compute the number of physical slices that can fit in the remaining space after
        // accounting for the metadata.
        metadata.header.pslice_count =
            (disk_size.checked_sub(metadata.data_start()).ok_or(zx::Status::INVALID_ARGS)?)
                / slice_size;
        ensure!(metadata.header.pslice_count > 0, zx::Status::INVALID_ARGS);

        info!("Formatting FVM: {:?}", metadata.header);

        let block_client = block_client::RemoteBlockClient::new(block_proxy).await?;

        // Write both primary and secondary copies of the metadata.
        metadata.write(&block_client, metadata.offset_for_slot(0)).await?;
        metadata.write(&block_client, metadata.offset_for_slot(1)).await?;

        Ok(())
    }

    async fn handle_start(
        self: &Arc<Self>,
        device: ClientEnd<BlockMarker>,
        _options: StartOptions,
    ) -> Result<(), Error> {
        let mut fvm = Fvm::open(RemoteBlockClient::new(device.into_proxy()).await?).await?;

        for (&index, partition) in &fvm.inner.get_mut().metadata.partitions {
            self.add_volume_to_volumes_directory(index, &partition.name()).unwrap();
        }

        self.export_dir.add_entry_may_overwrite(
            "volumes",
            self.volumes_directory.clone(),
            /* overwrite: */ true,
        )?;

        *self.fvm.lock() = Some(Arc::new(fvm));
        Ok(())
    }

    fn add_volume_to_volumes_directory(
        self: &Arc<Self>,
        index: u16,
        name: &str,
    ) -> Result<(), Error> {
        let weak = Arc::downgrade(self);
        self.volumes_directory.add_entry(
            name,
            vfs::service::host(move |requests| {
                let weak = weak.clone();
                async move {
                    if let Some(me) = weak.upgrade() {
                        let _ = me.handle_volume_requests(requests, index).await;
                    }
                }
            }),
        )?;
        Ok(())
    }

    // NOTE: Only safe after `handle_start` has been called.
    fn fvm(&self) -> Arc<Fvm> {
        self.fvm.lock().as_ref().unwrap().clone()
    }

    async fn handle_volumes_requests(
        self: &Arc<Self>,
        mut stream: VolumesRequestStream,
    ) -> Result<(), Error> {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                VolumesRequest::Create {
                    responder,
                    name,
                    outgoing_directory,
                    create_options,
                    mount_options,
                } => {
                    info!(name:?, create_options:?; "Create volume");
                    responder.send(
                        self.handle_create_volume(
                            &name,
                            outgoing_directory,
                            create_options,
                            mount_options,
                        )
                        .await
                        .map_err(|error| {
                            log::warn!(error:?; "Create volume failed");
                            map_to_raw_status(error)
                        }),
                    )?;
                }
                VolumesRequest::Remove { responder, name } => {
                    info!(name:?; "Remove volume");
                    responder.send(self.handle_remove_volume(&name).await.map_err(|error| {
                        log::warn!(error:?; "Remove volume failed");
                        map_to_raw_status(error)
                    }))?;
                }
                VolumesRequest::GetInfo { responder } => {
                    responder.send(Ok(Some(&self.fvm().get_info().await)))?;
                }
            }
        }
        Ok(())
    }

    async fn handle_volume_requests(
        self: Arc<Self>,
        mut requests: VolumeRequestStream,
        partition_index: u16,
    ) -> Result<(), Error> {
        while let Some(request) = requests.try_next().await? {
            match request {
                VolumeRequest::Check { responder, options } => responder.send(
                    self.handle_volume_check(partition_index, options).await.map_err(|error| {
                        warn!(error:?, partition_index; "Failed to check volume");
                        map_to_status(error).into_raw()
                    }),
                )?,
                VolumeRequest::Mount { responder, outgoing_directory, options } => responder.send(
                    self.handle_volume_mount(partition_index, outgoing_directory, options, false)
                        .await
                        .map_err(|error| {
                            let error = map_to_status(error);
                            if error == zx::Status::WRONG_TYPE {
                                warn!(
                                    error:?,
                                    partition_index;
                                    "Volume had unexpected format. Not continuing mount."
                                );
                            } else {
                                error!(error:?, partition_index; "Failed to mount volume");
                            }
                            error.into_raw()
                        }),
                )?,
                VolumeRequest::SetLimit { responder, bytes } => {
                    responder.send(self.handle_set_limit(partition_index, bytes).await.map_err(
                        |error| {
                            error!(error:?, partition_index; "Failed to set partition byte limit");
                            zx::Status::INTERNAL.into_raw()
                        },
                    ))?;
                }
                VolumeRequest::GetLimit { responder } => {
                    let fvm = self.fvm();
                    let inner = fvm.inner.read().await;
                    responder.send(Ok(inner.partition_state[&partition_index].slice_limit
                        * inner.metadata.header.slice_size))?;
                }
                VolumeRequest::GetInfo { responder } => {
                    let guid = {
                        let fvm = self.fvm();
                        let inner = fvm.inner.read().await;
                        inner.metadata.partitions.get(&partition_index).unwrap().guid
                    };
                    let response = fidl_fuchsia_fs_startup::VolumeInfo {
                        guid: Some(guid),
                        ..Default::default()
                    };
                    responder.send(Ok(&response))?;
                }
            }
        }
        Ok(())
    }

    async fn handle_set_limit(&self, partition_index: u16, max_bytes: u64) -> Result<(), Error> {
        let fvm = self.fvm();
        let mut inner = fvm.inner.write().await;
        // slice_size cannot be zero since we check it when we first mount.
        let slice_size = inner.metadata.header.slice_size;
        let partition_state = inner.partition_state.get_mut(&partition_index).unwrap();
        partition_state.slice_limit = max_bytes / slice_size;
        partition_state.partition_inspect.update_max_bytes(max_bytes);
        Ok(())
    }

    async fn create_device_for_partition(
        &self,
        partition_index: u16,
        crypt: Option<ClientEnd<CryptMarker>>,
        key_for_formatting: bool,
    ) -> Result<BlockServer<SessionManager<PartitionInterface>>, Error> {
        let fvm = self.fvm();
        let device_info = {
            let inner = fvm.inner.read().await;
            let partition =
                &inner.metadata.partitions.get(&partition_index).ok_or(zx::Status::INTERNAL)?;
            DeviceInfo::Volume(VolumeInfo {
                device_flags: fvm.device.block_flags(),
                max_transfer_blocks: fvm.device.max_transfer_blocks(),
                type_guid: partition.type_guid,
                instance_guid: partition.guid,
                name: partition.name().to_string(),
                flags: 0,
            })
        };
        let key = if let Some(crypt) = crypt {
            let crypt_proxy = crypt.into_proxy();
            Some(if key_for_formatting {
                info!(partition_index:?; "formatting zxcrypt on device");
                zxcrypt::Key::format(&fvm, partition_index, &crypt_proxy).await.unwrap()
            } else {
                info!(partition_index:?; "unsealing zxcrypt on device");
                zxcrypt::Key::unseal(&fvm, partition_index, &crypt_proxy).await?
            })
        } else {
            None
        };

        Ok(BlockServer::new(
            fvm.block_size(),
            Arc::new(PartitionInterface { partition_index, device_info, key, fvm: fvm.clone() }),
        ))
    }

    async fn mount_volume(
        &self,
        block_server: BlockServer<SessionManager<PartitionInterface>>,
        uri: &str,
        start_options: StartOptions,
        check_disk_format: bool,
    ) -> Result<(MountedVolume, Filesystem), Error> {
        // For now we only support URIs of the form: "#meta/<component_name>.cm".
        let Some(component) = uri
            .strip_prefix("#meta/")
            .and_then(|s| s.strip_suffix(".cm"))
            .filter(|s| !s.is_empty())
        else {
            bail!(zx::Status::INVALID_ARGS);
        };

        struct ComponentName(String, StartOptions);
        impl FSConfig for ComponentName {
            fn options(&self) -> Options<'_> {
                Options {
                    component_name: &self.0,
                    reuse_component_after_serving: false,
                    format_options: FormatOptions::default(),
                    start_options: self.1.clone(),
                    component_type: ComponentType::DynamicChild {
                        collection_name: if self.0 == "blobfs" {
                            // TODO(https://fxbug.dev/473793632): Move blobfs to fs-collection.
                            "blobfs-collection".to_string()
                        } else {
                            fs_management::FS_COLLECTION_NAME.to_string()
                        },
                    },
                }
            }
        }

        let volume = MountedVolume::new(block_server);
        if check_disk_format {
            let expected_format = match component {
                "minfs" => Some(DiskFormat::Minfs),
                "f2fs" => Some(DiskFormat::F2fs),
                _ => None,
            };
            // It's relatively normal to try to mount a filesystem, discover it's not the right
            // format, and reformat it so it is. If this filesystem is supposed to be a data
            // filesystem, spot-check that this partition is the right format before we try serving,
            // and return the proper error without logging scary things if it is not.
            if let Some(expected_format) = expected_format {
                let block_proxy = volume.connect_block()?.into_proxy();
                let detected_format = detect_disk_format(&block_proxy).await;
                ensure!(detected_format == expected_format, zx::Status::WRONG_TYPE);
            }
        }
        let fs =
            Filesystem::new(volume.clone(), ComponentName(component.to_string(), start_options));
        Ok((volume, fs))
    }

    async fn handle_volume_check(
        self: &Arc<Self>,
        partition_index: u16,
        options: CheckOptions,
    ) -> Result<(), Error> {
        let block_server =
            self.create_device_for_partition(partition_index, options.crypt, false).await?;
        let (_, mut fs) = self
            .mount_volume(
                block_server,
                &options.uri.ok_or(zx::Status::INVALID_ARGS).context("No URI specified")?,
                StartOptions { read_only: Some(true), ..Default::default() },
                true,
            )
            .await?;
        fs.fsck().await
    }

    async fn handle_volume_mount(
        self: &Arc<Self>,
        partition_index: u16,
        server_end: ServerEnd<fio::DirectoryMarker>,
        options: MountOptions,
        format: bool,
    ) -> Result<(), Error> {
        let block_server =
            self.create_device_for_partition(partition_index, options.crypt, format).await?;

        if let Some(uri) = options.uri {
            let (volume, mut fs) =
                self.mount_volume(block_server, &uri, StartOptions::default(), !format).await?;
            let volume_scope = volume.scope.clone();

            if format {
                fs.format().await?;
            }

            // TODO(https://fxbug.dev/357467643): Support properly shutting down the filesystem.
            // For now, just leak the mounted filesystem.
            let exposed_dir = fs.serve().await?.take_exposed_dir();

            self.mounted.lock().insert(partition_index, volume);

            let outgoing_dir = vfs::directory::immutable::simple();
            // TODO(https://fxbug.dev/392184892): fxfs volumes have an svc directory instead of the
            // flat hierarchy a component exposed dir has. We temporarily mimic this structure by
            // putting the whole exposed dir into an svc directory.
            let (svc_proxy, svc_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            outgoing_dir.add_entry("svc", vfs::remote::remote_dir(svc_proxy))?;
            exposed_dir.open(
                ".",
                fio::PERM_READABLE
                    | fio::Flags::PERM_INHERIT_EXECUTE
                    | fio::Flags::PERM_INHERIT_WRITE,
                &fio::Options::default(),
                svc_server_end.into_channel(),
            )?;
            let (root_proxy, root_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            outgoing_dir.add_entry("root", vfs::remote::remote_dir(root_proxy))?;
            exposed_dir.open(
                "root",
                fio::PERM_READABLE
                    | fio::Flags::PERM_INHERIT_EXECUTE
                    | fio::Flags::PERM_INHERIT_WRITE,
                &fio::Options::default(),
                root_server_end.into_channel(),
            )?;
            vfs::directory::serve_on(
                outgoing_dir,
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
                volume_scope,
                server_end,
            );
        } else {
            // Expose the volume as a block device.
            let outgoing_dir = vfs::directory::immutable::simple();
            let svc_dir = vfs::directory::immutable::simple();
            outgoing_dir.add_entry("svc", svc_dir.clone())?;
            let weak = Arc::downgrade(self);
            svc_dir.add_entry(
                fblock::BlockMarker::PROTOCOL_NAME,
                vfs::service::host(move |requests| {
                    let weak = weak.clone();
                    async move {
                        if let Some(me) = weak.upgrade() {
                            let _ = me.handle_volume(partition_index, requests).await;
                        }
                    }
                }),
            )?;

            let volume = MountedVolume::new(block_server);
            let volume_clone = volume.clone();
            let volume_scope = volume.scope.clone();
            self.mounted.lock().insert(partition_index, volume);

            vfs::directory::serve_on(
                outgoing_dir,
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
                volume_scope,
                server_end,
            );

            // Unmount when the last connection is closed (i.e. when `scope` terminates). This
            // needs to happen after we set up a connection on the scope or it might shut down the
            // volume before we connect anything to it.
            let this = self.clone();
            fasync::Task::spawn(async move {
                volume_clone.scope.wait().await;
                if !volume_clone.shutdown.swap(true, Ordering::Relaxed) {
                    this.mounted.lock().remove(&partition_index);
                }
            })
            .detach();
        }

        Ok(())
    }

    async fn handle_volume(
        self: Arc<Self>,
        partition_index: u16,
        requests: fblock::BlockRequestStream,
    ) -> Result<(), Error> {
        let partition = self.mounted.lock()[&partition_index].clone();
        partition.block_server.handle_requests(requests).await
    }

    async fn handle_create_volume(
        self: &Arc<Self>,
        name: &str,
        outgoing_directory: ServerEnd<fio::DirectoryMarker>,
        create_options: CreateOptions,
        mount_options: MountOptions,
    ) -> Result<(), Error> {
        let fvm = self.fvm.lock().as_ref().unwrap().clone();
        let inner = fvm.inner.upgradable_read().await;
        let Some(type_guid) = create_options.type_guid else {
            return Err(anyhow!(zx::Status::INVALID_ARGS).context("Missing type GUID"));
        };
        let guid = create_options.guid.unwrap_or_else(|| Uuid::new_v4().to_bytes_le());
        let slices = match create_options.initial_size {
            Some(x) => {
                if x % inner.metadata.header.slice_size > 0 {
                    return Err(anyhow!(zx::Status::INVALID_ARGS)).with_context(|| {
                        format!(
                            "Invalid volume size: size {} not a multiple of slice size {}",
                            x, inner.metadata.header.slice_size
                        )
                    });
                }
                (x / inner.metadata.header.slice_size).try_into().map_err(|_| {
                    anyhow!(zx::Status::INVALID_ARGS).context("Invalid volume size: too large")
                })?
            }
            None => 1,
        };
        let partition_index = fvm
            .create_partition(inner, type_guid, guid, slices, name)
            .await
            .context("Failed to create partition")?;

        async move {
            self.add_volume_to_volumes_directory(partition_index, name)?;

            self.handle_volume_mount(partition_index, outgoing_directory, mount_options, true).await
        }
        .await
        .map_err(|error| {
            warn!(error:?; "Created partition {name}, but failed to mount");
            error
        })
    }

    async fn handle_remove_volume(self: &Arc<Self>, name: &str) -> Result<(), Error> {
        let fvm = self.fvm.lock().as_ref().unwrap().clone();
        let inner = fvm.inner.upgradable_read().await;
        let Some((&partition_index, _)) =
            inner.metadata.partitions.iter().find(|(_, p)| p.name() == name)
        else {
            bail!(zx::Status::NOT_FOUND);
        };
        let volume = self.mounted.lock().get(&partition_index).cloned();
        if let Some(volume) = volume {
            volume.scope.shutdown();
            volume.scope.wait().await;
            if !volume.shutdown.swap(true, Ordering::Relaxed) {
                self.mounted.lock().remove(&partition_index);
            }
        }
        let mut new_metadata = inner.metadata.clone();
        let mut removed_slices = 0u64;
        for slice_entry in &mut new_metadata.allocations {
            if slice_entry.partition_index() == partition_index {
                slice_entry.0 = 0;
                removed_slices += 1;
            }
        }
        if new_metadata.partitions.remove(&partition_index).unwrap().slices as u64 != removed_slices
        {
            warn!("Mismatch between removed slices and assigned slices");
        }
        let mut inner = fvm.write_new_metadata(inner, new_metadata).await?;
        inner.assigned_slice_count -= removed_slices;
        inner.partition_state.remove(&partition_index);

        if let Err(error) = self.volumes_directory.remove_entry(name, false) {
            warn!(error:?; "Removed volume from FVM, but failed to remove entry from directory");
        }

        Ok(())
    }

    async fn handle_admin_requests(
        self: &Arc<Self>,
        mut stream: AdminRequestStream,
    ) -> Result<(), Error> {
        match stream.try_next().await? {
            None => Ok(()),
            Some(AdminRequest::Shutdown { responder }) => {
                info!("Received admin shutdown request");
                // Shut down any remaining volumes.
                let volumes = self.mounted.lock();
                for volume in volumes.values() {
                    volume.scope.shutdown();
                }
                // Shut down the main scope. The main thread is waiting on this shutdown to exit,
                // so it should be the ~last thing we do.
                self.scope.shutdown();
                responder
                    .send()
                    .unwrap_or_else(|error| warn!(error:?; "Failed to send shutdown response"));
                info!("Shutdown complete");
                Ok(())
            }
        }
    }

    async fn handle_reset_requests(
        self: &Arc<Self>,
        mut stream: ResetRequestStream,
    ) -> Result<(), Error> {
        match stream.try_next().await? {
            None => Ok(()),
            Some(ResetRequest::ShredEncryptedVolumes { responder }) => {
                info!("Received reset shred encrypted volumes request");
                responder
                    .send(self.handle_shred_encrypted_volumes().await.map_err(|error| {
                        warn!(error:?; "Failed to shred encrypted volumes");
                        zx::Status::INTERNAL.into_raw()
                    }))
                    .unwrap_or_else(|error| warn!(error:?; "Failed to send shred response"));
                info!("Shred encrypted volumes complete");
                Ok(())
            }
        }
    }

    async fn handle_shred_encrypted_volumes(self: &Arc<Self>) -> Result<(), Error> {
        let fvm = self.fvm();
        let inner = fvm.inner.read().await;
        for partition_index in inner.metadata.partitions.keys() {
            Key::shred_if_zxcrypt_volume(&fvm, *partition_index).await?;
        }

        Ok(())
    }
}

struct PartitionInterface {
    partition_index: u16,
    device_info: DeviceInfo,
    key: Option<zxcrypt::Key>,
    fvm: Arc<Fvm>,
}

impl Interface for PartitionInterface {
    async fn on_attach_vmo(&self, vmo: &zx::Vmo) -> Result<(), zx::Status> {
        // For encrypted partitions, we avoid exposing plaintext to the underlying block driver.
        // Instead of sending client VMOs directly to the driver, all I/O is bounced through local,
        // private staging buffers in zxcrypt where data is encrypted/decrypted out of place.
        // Only the local staging VMO containing ciphertext is registered with the underlying
        // device, so attaching client VMOs is a no-op.
        if self.key.is_some() { Ok(()) } else { self.fvm.device.attach_vmo(vmo).await }
    }

    fn on_detach_vmo(&self, vmo: &zx::Vmo) {
        if self.key.is_some() {
            // No-op: client VMOs are never attached to the underlying device when encrypted
            // because I/O uses local staging buffers to avoid exposing plaintext to the driver.
        } else {
            self.fvm.device.detach_vmo(vmo);
        }
    }

    fn get_info(&self) -> Cow<'_, DeviceInfo> {
        Cow::Borrowed(&self.device_info)
    }

    async fn read(
        &self,
        device_block_offset: u64,
        block_count: u32,
        vmo: &Arc<zx::Vmo>,
        vmo_offset: u64,
        _opts: ReadOptions,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        debug!(
            "read {}: @{device_block_offset}, count={block_count}, vmo_offset={vmo_offset}",
            self.partition_index
        );
        let device = &self.fvm.device;
        if let Some(key) = &self.key {
            let device_block_offset = device_block_offset
                .checked_add(self.fvm.slice_blocks)
                .ok_or(zx::Status::OUT_OF_RANGE)?;
            self.fvm
                .do_io(
                    zxcrypt::EncryptedRead::new(
                        &device,
                        &key,
                        device_block_offset,
                        vmo.clone(),
                        vmo_offset,
                    )
                    .await,
                    self.partition_index,
                    device_block_offset,
                    block_count,
                    trace_id(trace_flow_id),
                )
                .await
        } else {
            self.fvm
                .do_io(
                    Read {
                        device: &device,
                        vmo_id: &device.get_vmo_id(&vmo),
                        ops: FuturesUnordered::new(),
                        vmo_offset,
                    },
                    self.partition_index,
                    device_block_offset,
                    block_count,
                    trace_id(trace_flow_id),
                )
                .await
        }
        .map_err(|error| {
            warn!(error:?; "Read failed");
            map_to_status(error)
        })
    }

    async fn write(
        &self,
        device_block_offset: u64,
        block_count: u32,
        vmo: &Arc<zx::Vmo>,
        vmo_offset: u64,
        options: WriteOptions,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        debug!(
            "write {}: @{device_block_offset}, count={block_count}, vmo_offset={vmo_offset}",
            self.partition_index
        );
        // Punting barrier support on the FVM.
        if options.flags.contains(WriteFlags::PRE_BARRIER) {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        let device = &self.fvm.device;
        if let Some(key) = &self.key {
            let device_block_offset = device_block_offset
                .checked_add(self.fvm.slice_blocks)
                .ok_or(zx::Status::OUT_OF_RANGE)?;
            self.fvm
                .do_io(
                    zxcrypt::EncryptedWrite::new(
                        &device,
                        &key,
                        device_block_offset,
                        vmo.clone(),
                        vmo_offset,
                        options,
                    )
                    .await,
                    self.partition_index,
                    device_block_offset,
                    block_count,
                    trace_id(trace_flow_id),
                )
                .await
        } else {
            let vmo_id = device.get_vmo_id(&vmo);
            self.fvm
                .do_io(
                    Write {
                        device,
                        vmo_id: &vmo_id,
                        ops: FuturesUnordered::new(),
                        options,
                        vmo_offset,
                    },
                    self.partition_index,
                    device_block_offset,
                    block_count,
                    trace_id(trace_flow_id),
                )
                .await
        }
        .map_err(|error| {
            warn!(error:?; "Write failed");
            map_to_status(error)
        })
    }

    async fn flush(&self, trace_flow_id: Option<NonZero<u64>>) -> Result<(), zx::Status> {
        self.fvm.device.flush_traced(trace_id(trace_flow_id)).await
    }

    async fn trim(
        &self,
        device_block_offset: u64,
        block_count: u32,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        let device = &self.fvm.device;
        self.fvm
            .do_io(
                Trim { device, ops: FuturesUnordered::new() },
                self.partition_index,
                device_block_offset,
                block_count,
                trace_id(trace_flow_id),
            )
            .await
            .map_err(|error| {
                warn!(error:?; "Trim failed");
                map_to_status(error)
            })
    }

    async fn get_volume_info(
        &self,
    ) -> Result<(fblock::VolumeManagerInfo, fblock::VolumeInfo), zx::Status> {
        let volume_manager_info = self.fvm.get_info().await;
        let inner = self.fvm.inner.read().await;
        let reserved = if self.key.is_some() { 1 } else { 0 };
        Ok((
            volume_manager_info,
            fblock::VolumeInfo {
                partition_slice_count: (inner
                    .metadata
                    .partitions
                    .get(&self.partition_index)
                    .unwrap()
                    .slices as u64)
                    .saturating_sub(reserved),
                slice_limit: inner.partition_state[&self.partition_index]
                    .slice_limit
                    .saturating_sub(reserved),
            },
        ))
    }

    async fn query_slices(
        &self,
        start_slices: &[u64],
    ) -> Result<Vec<fblock::VsliceRange>, zx::Status> {
        let inner = self.fvm.inner.read().await;
        let mappings = &inner.partition_state[&self.partition_index].mappings;
        let mut results = Vec::new();
        for &(mut slice) in start_slices {
            if self.key.is_some() {
                slice = slice.checked_add(1).ok_or(zx::Status::OUT_OF_RANGE)?;
            }
            if slice >= MAX_SLICE_COUNT {
                return Err(zx::Status::OUT_OF_RANGE);
            }
            let index = match mappings.binary_search_by(|m: &Mapping| m.from.start.cmp(&slice)) {
                Ok(index) => index,
                Err(index) if index > 0 => index - 1,
                _ => {
                    results.push(fblock::VsliceRange {
                        allocated: false,
                        count: MAX_SLICE_COUNT - slice,
                    });
                    continue;
                }
            };
            let mapping = &mappings[index];
            let mut end_slice = mapping.from.end;
            if slice >= end_slice {
                if index + 1 < mappings.len() {
                    results.push(fblock::VsliceRange {
                        allocated: false,
                        count: mappings[index + 1].from.start - slice,
                    });
                } else {
                    results.push(fblock::VsliceRange {
                        allocated: false,
                        count: MAX_SLICE_COUNT - slice,
                    });
                }
            } else {
                // Coalesce mappings.
                for mapping in &mappings[index + 1..] {
                    if mapping.from.start != end_slice {
                        break;
                    }
                    end_slice = mapping.from.end;
                }
                results.push(fblock::VsliceRange { allocated: true, count: end_slice - slice });
            }
        }
        Ok(results)
    }

    async fn extend(&self, start_slice: u64, slice_count: u64) -> Result<(), zx::Status> {
        bounds_check_slice_extent(start_slice, slice_count)?;
        if slice_count == 0 {
            return Ok(());
        }
        let start_slice = if self.key.is_some() {
            start_slice.checked_add(1).ok_or(zx::Status::OUT_OF_RANGE)?
        } else {
            start_slice
        };

        let inner = self.fvm.inner.upgradable_read().await;

        let partition_state = &inner.partition_state[&self.partition_index];
        let mappings = &partition_state.mappings;

        let check_already_allocated =
            || match mappings.binary_search_by(|m: &Mapping| m.from.start.cmp(&start_slice)) {
                Ok(_) => Err(zx::Status::INVALID_ARGS),
                Err(index) => {
                    if (index > 0 && mappings[index - 1].from.end > start_slice)
                        || (index < mappings.len()
                            && mappings[index].from.start - start_slice < slice_count)
                    {
                        Err(zx::Status::INVALID_ARGS)
                    } else {
                        Ok(index)
                    }
                }
            };

        if let Err(e) = check_already_allocated() {
            warn!("Attempt to allocate vslice {start_slice} that is already allocated.");
            return Err(e);
        }

        if partition_state.slice_limit > 0
            && (inner.metadata.partitions[&self.partition_index].slices as u64)
                .checked_add(slice_count)
                .map_or(true, |s| s > partition_state.slice_limit)
        {
            return Err(zx::Status::NO_SPACE);
        }

        let mut new_metadata = inner.metadata.clone();
        let new_mappings =
            new_metadata.allocate_slices(self.partition_index, start_slice, slice_count)?;

        let mut inner =
            self.fvm.write_new_metadata(inner, new_metadata).await.map_err(map_to_status)?;

        inner.assigned_slice_count = inner.assigned_slice_count.checked_add(slice_count).unwrap();

        let total_slices = inner.metadata.partitions[&self.partition_index].slices as u64;
        let partition_state = inner.partition_state.get_mut(&self.partition_index).unwrap();
        partition_state.mappings.insert_contiguous_mappings(new_mappings);
        partition_state.partition_inspect.update_total_slices(total_slices);

        Ok(())
    }

    async fn shrink(&self, start_slice: u64, slice_count: u64) -> Result<(), zx::Status> {
        bounds_check_slice_extent(start_slice, slice_count)?;
        if slice_count == 0 {
            return Ok(());
        }
        let start_slice = if self.key.is_some() {
            start_slice.checked_add(1).ok_or(zx::Status::OUT_OF_RANGE)?
        } else {
            start_slice
        };
        let inner = self.fvm.inner.upgradable_read().await;
        let mappings = &inner.partition_state[&self.partition_index].mappings;

        let mut new_metadata = inner.metadata.clone();
        let end_slice = start_slice.checked_add(slice_count).ok_or(zx::Status::INVALID_ARGS)?;
        let mut deallocated_count: u64 = 0;

        // Deallocate physical slices.
        for mapping in mappings.iter() {
            if mapping.from.start >= end_slice {
                break;
            }
            let dealloc_start = std::cmp::max(start_slice, mapping.from.start);
            let dealloc_end = std::cmp::min(end_slice, mapping.from.end);

            if dealloc_end > dealloc_start {
                let count = dealloc_end - dealloc_start;
                let offset = dealloc_start - mapping.from.start;
                let start_physical_slice = (mapping.to + offset) as usize;
                let end_physical_slice = start_physical_slice + count as usize;
                new_metadata.allocations[start_physical_slice..end_physical_slice]
                    .fill(SliceEntry(0));
                deallocated_count += count;
            }
        }

        if deallocated_count == 0 {
            return Err(zx::Status::INVALID_ARGS);
        }

        // Write metadata changes to the device.
        let slices = &mut new_metadata.partitions.get_mut(&self.partition_index).unwrap().slices;
        *slices = slices.saturating_sub(deallocated_count as u32);

        let mut inner =
            self.fvm.write_new_metadata(inner, new_metadata).await.map_err(map_to_status)?;

        inner.assigned_slice_count =
            inner.assigned_slice_count.checked_sub(deallocated_count).unwrap();

        // Update the in-memory mappings.
        let total_slices = inner.metadata.partitions[&self.partition_index].slices as u64;
        let partition_state = inner.partition_state.get_mut(&self.partition_index).unwrap();
        partition_state.partition_inspect.update_total_slices(total_slices);
        let mappings = &mut partition_state.mappings;

        // Find the range of mappings that overlap with the shrink range.
        let start_idx = mappings.partition_point(|m| m.from.end <= start_slice);
        let end_idx = mappings.partition_point(|m| m.from.start < end_slice);

        if start_idx >= end_idx {
            // This should not be reached if deallocated_count > 0, but as a safeguard:
            return Ok(());
        }

        let mut replacements = Vec::new();
        let first_affected_mapping = &mappings[start_idx];
        if first_affected_mapping.from.start < start_slice {
            // The first affected mapping is split. Keep the part before the shrink range.
            replacements.push(Mapping {
                from: first_affected_mapping.from.start..start_slice,
                to: first_affected_mapping.to,
            });
        }

        let last_affected_mapping = &mappings[end_idx - 1];
        if last_affected_mapping.from.end > end_slice {
            // The last affected mapping is split. Keep the part after the shrink range.
            replacements.push(Mapping {
                from: end_slice..last_affected_mapping.from.end,
                to: last_affected_mapping.to + (end_slice - last_affected_mapping.from.start),
            });
        }

        mappings.splice(start_idx..end_idx, replacements);

        Ok(())
    }
}

fn bounds_check_slice_extent(start_slice: u64, slice_count: u64) -> Result<(), zx::Status> {
    if start_slice == 0 {
        return Err(zx::Status::OUT_OF_RANGE); // 0 is an invalid slice number
    }

    match start_slice.checked_add(slice_count) {
        None => Err(zx::Status::OUT_OF_RANGE), // integer overflow
        Some(final_slice) => {
            if final_slice > MAX_SLICE_COUNT {
                Err(zx::Status::OUT_OF_RANGE) // End of request is outside addressable slice range
            } else {
                Ok(())
            }
        }
    }
}

fn map_to_raw_status(e: Error) -> zx::sys::zx_status_t {
    map_to_status(e).into_raw()
}

fn map_to_status(error: anyhow::Error) -> zx::Status {
    if let Some(status) = error.root_cause().downcast_ref::<zx::Status>() {
        status.clone()
    } else {
        // Print the internal error if we re-map it because we will lose any context after this.
        warn!(error:?; "Internal error");
        zx::Status::INTERNAL
    }
}

#[fuchsia::main(
    logging_tags = ["fvm"],
    threads = 2,
    thread_role = "fuchsia.devices.block.drivers.core.block-server")]
async fn main() -> Result<(), Error> {
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let component = Arc::new(Component::new());
    component
        .serve(
            fuchsia_runtime::take_startup_handle(HandleType::DirectoryRequest.into())
                .ok_or_else(|| anyhow!("Missing startup handle"))
                .unwrap()
                .into(),
        )
        .await
        .unwrap();
    component.scope.wait().await;
    Ok(())
}

/// Holds memory that has the same alignment as T, but with an arbitrary size.
struct AlignedMem<T>(&'static mut [u8], PhantomData<T>);

impl<T> AlignedMem<T> {
    fn new(size: usize) -> Self {
        Self(
            // SAFETY: Safe by inspection.
            unsafe {
                std::slice::from_raw_parts_mut(
                    alloc::alloc_zeroed(
                        alloc::Layout::from_size_align(size, std::mem::align_of::<T>()).unwrap(),
                    ),
                    size,
                )
            },
            PhantomData,
        )
    }
}

impl<T> Drop for AlignedMem<T> {
    fn drop(&mut self) {
        // SAFETY: Pairs with the allocation in `new`.
        unsafe {
            alloc::dealloc(
                self.0.as_mut_ptr(),
                alloc::Layout::from_size_align(self.0.len(), std::mem::align_of::<T>()).unwrap(),
            );
        }
    }
}

impl<T> Deref for AlignedMem<T> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0
    }
}

impl<T> DerefMut for AlignedMem<T> {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Component, Fvm, HeaderRaw, MAGIC, MAJOR_VERSION, MAX_PARTITIONS, MAX_SLICE_COUNT,
        MINOR_VERSION, Metadata, PartitionEntry, SliceEntry,
    };
    use assert_matches::assert_matches;
    use block_client::{
        BlockClient, BufferSlice, MutableBufferSlice, RemoteBlockClient, WriteFlags, WriteOptions,
    };
    use block_server::{BlockInfo, DeviceInfo};
    use fidl::endpoints::RequestStream;
    use fidl_fuchsia_fs::AdminMarker;
    use fidl_fuchsia_fs_startup::{
        CheckOptions, CreateOptions, FormatOptions, MountOptions, StartOptions, StartupMarker,
        VolumeMarker, VolumesMarker,
    };
    use fidl_fuchsia_fxfs::{CryptMarker, CryptRequest};
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_storage_block as fblock;
    use fidl_fuchsia_storage_block::BlockMarker;
    use fs_management::DATA_TYPE_GUID;
    use fs_management::format::constants::ZXCRYPT_MAGIC;
    use fuchsia_async as fasync;
    use fuchsia_component::client::{
        connect_to_named_protocol_at_dir_root, connect_to_protocol_at_dir_svc,
    };
    use fuchsia_fs::directory::{open_directory, readdir};
    use futures::TryStreamExt as _;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use test_vmo_backed_block_server::{
        InitialContents, Observer, VmoBackedServer, VmoBackedServerOptions, WriteAction, WriteCache,
    };

    struct Fixture {
        component: Arc<Component>,
        outgoing_dir: fio::DirectoryProxy,
        fake_server: Arc<VmoBackedServer>,
    }

    const BLOCK_SIZE: u32 = 512;
    const SLICE_SIZE: u64 = 32768;

    impl Fixture {
        async fn new(extra_space: u64) -> Self {
            let contents = std::fs::read("/pkg/data/golden-fvm.blk").unwrap();
            let fake_server = Arc::new(
                VmoBackedServer::new(
                    (contents.len() as u64 + extra_space) / BLOCK_SIZE as u64,
                    BLOCK_SIZE,
                    &contents,
                )
                .expect("Failed to create VmoBackedServer"),
            );
            Self::from_fake_server(fake_server).await
        }

        async fn from_fake_server(fake_server: Arc<VmoBackedServer>) -> Self {
            let (outgoing_dir, server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            let fixture =
                Fixture { component: Arc::new(Component::new()), outgoing_dir, fake_server };
            let fake_server = fixture.fake_server.clone();
            let (block_client, block_server) =
                fidl::endpoints::create_request_stream::<BlockMarker>();
            fasync::Task::spawn(async move {
                let _ = fake_server.serve(block_server.cast_stream()).await;
            })
            .detach();
            fixture.component.serve(server_end.into_channel()).await.unwrap();
            let startup_proxy =
                connect_to_protocol_at_dir_svc::<StartupMarker>(&fixture.outgoing_dir).unwrap();

            startup_proxy
                .start(block_client.into_channel().into(), &StartOptions::default())
                .await
                .expect("start failed (FIDL")
                .expect("start failed");

            fixture
        }

        async fn mount_volume(&self, volume_name: &str) -> fblock::BlockProxy {
            let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
                &self.outgoing_dir,
                &format!("volumes/{volume_name}"),
            )
            .unwrap();

            let (dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volume_proxy
                .mount(dir_server_end, MountOptions::default())
                .await
                .expect("mount failed (FIDL)")
                .expect("mount failed");

            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap()
        }

        fn take_fake_server(self) -> Arc<VmoBackedServer> {
            self.fake_server
        }
    }

    #[fuchsia::test]
    async fn test_format_and_mount() {
        let block_size: u32 = 512;
        let block_count: u64 = 102400; // 50MB
        let fake_server = Arc::new(
            VmoBackedServer::new(block_count, block_size, &[])
                .expect("Failed to create VmoBackedServer"),
        );

        let (outgoing_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let component = Arc::new(Component::new());

        let fake_server_clone = fake_server.clone();
        let (block_client, block_server) = fidl::endpoints::create_request_stream::<BlockMarker>();
        fasync::Task::spawn(async move {
            let _ = fake_server_clone.serve(block_server.cast_stream()).await;
        })
        .detach();

        component.serve(server_end.into_channel()).await.unwrap();

        let startup_proxy = connect_to_protocol_at_dir_svc::<StartupMarker>(&outgoing_dir).unwrap();

        // Format the volume
        let options =
            FormatOptions { fvm_slice_size: Some(SLICE_SIZE), ..FormatOptions::default() };
        startup_proxy.format(block_client.into_channel().into(), &options).await.unwrap().unwrap();

        // Now try to start it
        let (block_client, block_server) = fidl::endpoints::create_request_stream::<BlockMarker>();
        let fake_server_clone = fake_server.clone();
        fasync::Task::spawn(async move {
            let _ = fake_server_clone.serve(block_server.cast_stream()).await;
        })
        .detach();

        startup_proxy
            .start(block_client.into_channel().into(), &StartOptions::default())
            .await
            .unwrap()
            .unwrap();

        // Now try to add a volume
        let volumes_proxy = connect_to_protocol_at_dir_svc::<VolumesMarker>(&outgoing_dir).unwrap();
        let (_vol_dir, vol_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let create_options = CreateOptions {
            initial_size: Some(32768),
            type_guid: Some([1; 16]),
            ..CreateOptions::default()
        };
        volumes_proxy
            .create("default", vol_server, create_options, MountOptions::default())
            .await
            .unwrap()
            .unwrap();

        // Verify that it exists
        let volumes_dir =
            fuchsia_fs::directory::open_directory(&outgoing_dir, "volumes", fio::PERM_READABLE)
                .await
                .unwrap();
        let entries = readdir(&volumes_dir).await.unwrap();
        assert!(entries.iter().any(|e| e.name == "default"));
    }

    #[fuchsia::test]
    async fn test_allocation_persistence() {
        // We want to allocate *every* slice within the table. To do that, we need to format the
        // volume and then effectively resize the disk so that all the entries can be allocated.
        let block_size: u32 = 512;
        let initial_block_count: u64 = 65600;
        let final_block_count: u64 = 65900;

        let vmo = zx::Vmo::create_with_opts(
            zx::VmoOptions::RESIZABLE,
            final_block_count * block_size as u64,
        )
        .unwrap();
        let vmo_dup = vmo.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let vmo_dup = zx::Vmo::from(vmo_dup);

        vmo.set_size(initial_block_count * block_size as u64).unwrap();
        let fake_server = Arc::new(
            VmoBackedServer::from_vmo(block_size, vmo).expect("Failed to create VmoBackedServer"),
        );

        let (outgoing_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let component = Arc::new(Component::new());

        let fake_server_clone = fake_server.clone();
        let (block_client, block_server) = fidl::endpoints::create_request_stream::<BlockMarker>();
        fasync::Task::spawn(async move {
            let _ = fake_server_clone.serve(block_server.cast_stream()).await;
        })
        .detach();

        component.serve(server_end.into_channel()).await.unwrap();

        let startup_proxy = connect_to_protocol_at_dir_svc::<StartupMarker>(&outgoing_dir).unwrap();

        let options =
            FormatOptions { fvm_slice_size: Some(SLICE_SIZE), ..FormatOptions::default() };
        startup_proxy.format(block_client.into_channel().into(), &options).await.unwrap().unwrap();

        // Shutdown component to grow disk
        let admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(&outgoing_dir).unwrap();
        admin_proxy.shutdown().await.unwrap();

        // Grow disk
        vmo_dup.set_size(final_block_count * block_size as u64).unwrap();

        // Remount with larger disk
        let fake_server = Arc::new(
            VmoBackedServer::from_vmo(block_size, vmo_dup)
                .expect("Failed to create VmoBackedServer"),
        );
        let (outgoing_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let component = Arc::new(Component::new());

        let fake_server_clone = fake_server.clone();
        let (block_client, block_server) = fidl::endpoints::create_request_stream::<BlockMarker>();
        fasync::Task::spawn(async move {
            let _ = fake_server_clone.serve(block_server.cast_stream()).await;
        })
        .detach();

        component.serve(server_end.into_channel()).await.unwrap();
        let startup_proxy = connect_to_protocol_at_dir_svc::<StartupMarker>(&outgoing_dir).unwrap();
        startup_proxy
            .start(block_client.into_channel().into(), &StartOptions::default())
            .await
            .unwrap()
            .unwrap();

        // Add a volume
        let volumes_proxy = connect_to_protocol_at_dir_svc::<VolumesMarker>(&outgoing_dir).unwrap();
        let (vol_dir, vol_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let create_options = CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() };
        volumes_proxy
            .create("default", vol_server, create_options, MountOptions::default())
            .await
            .unwrap()
            .unwrap();

        // Get volume proxy
        let vol_proxy = connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&vol_dir).unwrap();

        // Allocate 1023 MORE slices (it starts with 1). Total 1024.
        let status = vol_proxy.extend(2, 1023).await.unwrap();
        zx::Status::ok(status).unwrap();

        // Verify they are allocated
        let (_, manager_info, _) = vol_proxy.get_volume_info().await.unwrap();
        assert_eq!(manager_info.unwrap().assigned_slice_count, 1024);

        // Unmount
        let admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(&outgoing_dir).unwrap();
        admin_proxy.shutdown().await.unwrap();

        // Remount again
        let component = Arc::new(Component::new());
        let (outgoing_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let (block_client, block_server) = fidl::endpoints::create_request_stream::<BlockMarker>();
        let fake_server_clone = fake_server.clone();
        fasync::Task::spawn(async move {
            let _ = fake_server_clone.serve(block_server.cast_stream()).await;
        })
        .detach();

        component.serve(server_end.into_channel()).await.unwrap();
        let startup_proxy = connect_to_protocol_at_dir_svc::<StartupMarker>(&outgoing_dir).unwrap();
        startup_proxy
            .start(block_client.into_channel().into(), &StartOptions::default())
            .await
            .unwrap()
            .unwrap();

        // Get volume proxy again
        let vol_proxy = connect_to_named_protocol_at_dir_root::<
            fidl_fuchsia_fs_startup::VolumeMarker,
        >(&outgoing_dir, "volumes/default")
        .unwrap();
        let (vol_dir, vol_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        vol_proxy.mount(vol_server, MountOptions::default()).await.unwrap().unwrap();
        let vol_proxy = connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&vol_dir).unwrap();

        // Verify they are STILL allocated
        let (_, manager_info, _) = vol_proxy.get_volume_info().await.unwrap();
        assert_eq!(manager_info.unwrap().assigned_slice_count, 1024);
    }

    #[fuchsia::test]
    async fn test_grow_with_padding_garbage() {
        let block_size: u32 = 512;
        let initial_block_count: u64 = 65600;
        let final_block_count: u64 = 65900;

        let vmo = zx::Vmo::create_with_opts(
            zx::VmoOptions::RESIZABLE,
            final_block_count * block_size as u64,
        )
        .unwrap();
        let vmo_dup = vmo.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let vmo_dup = zx::Vmo::from(vmo_dup);

        vmo.set_size(initial_block_count * block_size as u64).unwrap();
        let fake_server = Arc::new(
            VmoBackedServer::from_vmo(block_size, vmo).expect("Failed to create VmoBackedServer"),
        );

        let (outgoing_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let component = Arc::new(Component::new());

        let fake_server_clone = fake_server.clone();
        let (block_client, block_server) = fidl::endpoints::create_request_stream::<BlockMarker>();
        fasync::Task::spawn(async move {
            let _ = fake_server_clone.serve(block_server.cast_stream()).await;
        })
        .detach();

        component.serve(server_end.into_channel()).await.unwrap();

        let startup_proxy = connect_to_protocol_at_dir_svc::<StartupMarker>(&outgoing_dir).unwrap();

        let options =
            FormatOptions { fvm_slice_size: Some(SLICE_SIZE), ..FormatOptions::default() };
        startup_proxy.format(block_client.into_channel().into(), &options).await.unwrap().unwrap();

        // Open FVM directly using Fvm::open
        let (block_client_client_end, block_server) =
            fidl::endpoints::create_request_stream::<BlockMarker>();
        let fake_server_clone = fake_server.clone();
        fasync::Task::spawn(async move {
            let _ = fake_server_clone.serve(block_server.cast_stream()).await;
        })
        .detach();

        let block_client =
            RemoteBlockClient::new(block_client_client_end.into_proxy()).await.unwrap();
        let fvm = Fvm::open(block_client).await.unwrap();

        // Write garbage to padding (slice 1020).
        // Point to partition 1 (which doesn't exist).
        {
            let mut inner = fvm.inner.write().await;
            let mut new_metadata = inner.metadata.clone();

            new_metadata.allocations.resize(1023, SliceEntry(0));

            // Set garbage at physical slice 1020 (index 1019) pointing to partition 1, logical
            // slice 1
            new_metadata.allocations[1019].set(1, 1);

            inner.write_new_metadata_to(&fvm.device, new_metadata).await.unwrap();
        }

        // Drop fvm to close it
        std::mem::drop(fvm);

        // Grow disk
        vmo_dup.set_size(final_block_count * block_size as u64).unwrap();

        let fake_server = VmoBackedServer::from_vmo(block_size, vmo_dup)
            .expect("Failed to create VmoBackedServer");
        let (block_client_client_end, block_server) =
            fidl::endpoints::create_request_stream::<BlockMarker>();
        fasync::Task::spawn(async move {
            let _ = fake_server.serve(block_server.cast_stream()).await;
        })
        .detach();

        let block_client =
            RemoteBlockClient::new(block_client_client_end.into_proxy()).await.unwrap();
        let fvm = Fvm::open(block_client).await.unwrap();
        let inner = fvm.inner.read().await;

        // The garbage we wrote should have been zeroed when FVM grew the number of slices.
        assert_eq!(inner.metadata.allocations[1019].partition_index(), 0);
    }

    #[fuchsia::test]
    async fn test_shrink_disk() {
        // We want to grow the disk to be larger than the allocation table will allow.
        let block_size: u32 = 512;

        // Start with enough blocks so that it uses more than one allocation block.  The metadata
        // should be M = 2 * (8194 + 65536 + 16384), and we can fit 1024 allocation entries in an 8
        // KiB block, so we need more than (M + 32768 * 1024) / 512 blocks.
        let initial_block_count: u64 = 70000;

        // This should result in just one data slice (M + 32768) / 512.
        let final_block_count: u64 = 416;

        let vmo = zx::Vmo::create_with_opts(
            zx::VmoOptions::RESIZABLE,
            final_block_count * block_size as u64,
        )
        .unwrap();
        let vmo_dup = vmo.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let vmo_dup = zx::Vmo::from(vmo_dup);

        vmo.set_size(initial_block_count * block_size as u64).unwrap();
        let fake_server =
            VmoBackedServer::from_vmo(block_size, vmo).expect("Failed to create VmoBackedServer");

        let (outgoing_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let component = Arc::new(Component::new());

        let (block_client, block_server) = fidl::endpoints::create_request_stream::<BlockMarker>();
        fasync::Task::spawn(async move {
            let _ = fake_server.serve(block_server.cast_stream()).await;
        })
        .detach();

        component.serve(server_end.into_channel()).await.unwrap();

        let startup_proxy = connect_to_protocol_at_dir_svc::<StartupMarker>(&outgoing_dir).unwrap();

        let options =
            FormatOptions { fvm_slice_size: Some(SLICE_SIZE), ..FormatOptions::default() };
        startup_proxy.format(block_client.into_channel().into(), &options).await.unwrap().unwrap();

        // Shutdown component to grow disk
        let admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(&outgoing_dir).unwrap();
        admin_proxy.shutdown().await.unwrap();

        // Shrink disk
        vmo_dup.set_size(final_block_count * block_size as u64).unwrap();

        // Remount with larger disk
        let fake_server = VmoBackedServer::from_vmo(block_size, vmo_dup)
            .expect("Failed to create VmoBackedServer");
        let (outgoing_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let component = Arc::new(Component::new());

        let (block_client, block_server) = fidl::endpoints::create_request_stream::<BlockMarker>();
        fasync::Task::spawn(async move {
            let _ = fake_server.serve(block_server.cast_stream()).await;
        })
        .detach();

        component.serve(server_end.into_channel()).await.unwrap();
        let startup_proxy = connect_to_protocol_at_dir_svc::<StartupMarker>(&outgoing_dir).unwrap();
        startup_proxy
            .start(block_client.into_channel().into(), &StartOptions::default())
            .await
            .unwrap()
            .unwrap();

        // Add a volume
        let volumes_proxy = connect_to_protocol_at_dir_svc::<VolumesMarker>(&outgoing_dir).unwrap();
        let (_vol_dir, vol_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let create_options = CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() };
        volumes_proxy
            .create("default", vol_server, create_options, MountOptions::default())
            .await
            .unwrap()
            .unwrap();
    }

    /// Returns the number of assigned slices for the individual volume and the whole FVM volume.
    async fn get_counts(proxy: &fblock::BlockProxy) -> (u64, u64) {
        let (status, manager_info, volume_info) = proxy.get_volume_info().await.unwrap();
        assert_eq!(status, zx::sys::ZX_OK);
        (manager_info.unwrap().assigned_slice_count, volume_info.unwrap().partition_slice_count)
    }

    #[fuchsia::test]
    async fn test_golden() {
        let fixture = Fixture::new(0).await;

        // Mount the blobfs partition.
        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/blobfs",
        )
        .unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        // Look for blobfs's magic:
        let block_proxy =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();
        let client = RemoteBlockClient::new(block_proxy).await.unwrap();

        // Make sure FVM passes through the right block size.
        assert_eq!(client.block_size(), BLOCK_SIZE);

        let mut buf = vec![0; 8192];
        client.read_at(MutableBufferSlice::Memory(&mut buf), 0).await.unwrap();

        const BLOBFS_MAGIC: &[u8] = &[
            0x21, 0x4d, 0x69, 0x9e, 0x47, 0x53, 0x21, 0xac, 0x14, 0xd3, 0xd3, 0xd4, 0xd4, 0x00,
            0x50, 0x98,
        ];

        assert_eq!(&buf[..16], BLOBFS_MAGIC);

        // And check the backup super-block:
        let mut buf = vec![0; 8192];
        client.read_at(MutableBufferSlice::Memory(&mut buf), 8192).await.unwrap();
        assert_eq!(&buf[..16], BLOBFS_MAGIC);

        // And check the journal magic, which is in a different slice:
        let mut buf = vec![0; 8192];
        client.read_at(MutableBufferSlice::Memory(&mut buf), 0x30000 * 8192).await.unwrap();
        assert_eq!(&buf[..8], &[0x6c, 0x6e, 0x72, 0x6a, 0x62, 0x6f, 0x6c, 0x62]);

        // Reading from a slice that's not allocated should fail.
        assert_eq!(
            client
                .read_at(MutableBufferSlice::Memory(&mut buf), SLICE_SIZE)
                .await
                .expect_err("Read from slice #2 should fail"),
            zx::Status::OUT_OF_RANGE
        );

        // Mount the minfs partition.
        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/data",
        )
        .unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let block_proxy =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();
        let client = RemoteBlockClient::new(block_proxy).await.unwrap();

        // Check some writes.
        let offsets = [0, 16384, 10 * 8192, 20 * 8192];
        for (index, &offset) in offsets.iter().enumerate() {
            let buf = vec![index as u8; 16384];
            client.write_at(BufferSlice::Memory(&buf), offset).await.unwrap();
        }

        // Read back in reverse.
        for (index, &offset) in offsets.iter().enumerate().rev() {
            let mut read_buf = vec![index as u8; 16384];
            client.read_at(MutableBufferSlice::Memory(&mut read_buf), offset).await.unwrap();
            assert_eq!(&read_buf, &[index as u8; 16384]);
        }
    }

    #[fuchsia::test]
    async fn test_create_volume() {
        let buf = vec![0xaf; 16384];

        let fake_server = {
            let fixture = Fixture::new(SLICE_SIZE).await;

            let volumes_proxy =
                connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

            let (dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volumes_proxy
                .create(
                    "foo",
                    dir_server_end,
                    CreateOptions {
                        type_guid: Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
                        ..CreateOptions::default()
                    },
                    MountOptions::default(),
                )
                .await
                .expect("create failed (FIDL)")
                .expect("create failed");

            // Check we can read and write from the new partition.
            let block_proxy =
                connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();
            let client = RemoteBlockClient::new(block_proxy).await.unwrap();

            // Check some writes.
            for offset in [0, 16384] {
                client.write_at(BufferSlice::Memory(&buf), offset).await.unwrap();
                let mut read_buf = vec![0; 16384];
                client.read_at(MutableBufferSlice::Memory(&mut read_buf), offset).await.unwrap();
                assert_eq!(&buf, &read_buf);
            }

            // Make sure the volume appears in the volumes directory.
            let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
                &fixture.outgoing_dir,
                "volumes/foo",
            )
            .unwrap();

            // Check we connected by calling the Check method.  It should fail as no URI was
            // specified
            volume_proxy
                .check(CheckOptions::default())
                .await
                .expect("FIDL error")
                .expect_err("Check volume should fail");

            fixture.fake_server
        };

        // Reopen, and check the same reads.
        let fixture = Fixture::from_fake_server(fake_server).await;

        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/foo",
        )
        .unwrap();
        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let block_proxy =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();
        let client = RemoteBlockClient::new(block_proxy).await.unwrap();

        for offset in [0, 16384] {
            let mut read_buf = vec![0; 16384];
            client.read_at(MutableBufferSlice::Memory(&mut read_buf), offset).await.unwrap();
            assert_eq!(&buf, &read_buf);
        }
    }

    #[fuchsia::test]
    async fn test_create_volume_no_space() {
        // On the first pass, we should run out of space due to lack of space for the partition
        // data, and in the second case, we should run out of space due to lack of space in the
        // partition table.
        for extra_space in [SLICE_SIZE, SLICE_SIZE * 1024] {
            // Keep creating partitions until we run out of space.
            let mut partition_count = 0;

            let fake_server = {
                let fixture = Fixture::new(extra_space).await;

                let volumes_proxy =
                    connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

                loop {
                    let (_dir_proxy, dir_server_end) =
                        fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                    match volumes_proxy
                        .create(
                            &format!("foo {partition_count}"),
                            dir_server_end,
                            CreateOptions {
                                type_guid: Some([
                                    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
                                ]),
                                ..CreateOptions::default()
                            },
                            MountOptions::default(),
                        )
                        .await
                        .expect("create failed (FIDL)")
                    {
                        Ok(()) => {}
                        Err(zx::sys::ZX_ERR_NO_SPACE) => break,
                        Err(error) => panic!("create failed: {error:?}"),
                    }
                    partition_count += 1;
                }
                fixture.fake_server
            };

            log::info!("Created {partition_count} partitions");

            // Reopen and check we can mount all the partitions we created.
            let fixture = Fixture::from_fake_server(fake_server).await;

            for i in 0..partition_count {
                let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
                    &fixture.outgoing_dir,
                    &format!("volumes/foo {i}"),
                )
                .unwrap();

                let (_dir_proxy, dir_server_end) =
                    fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                volume_proxy
                    .mount(dir_server_end, MountOptions::default())
                    .await
                    .expect("mount failed (FIDL)")
                    .expect("mount failed");
            }
        }
    }

    #[fuchsia::test]
    async fn test_remove_volume() {
        let fixture = Fixture::new(SLICE_SIZE).await;

        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

        let (_dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volumes_proxy
            .create(
                "foo",
                dir_server_end,
                CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() },
                MountOptions::default(),
            )
            .await
            .expect("create failed (FIDL)")
            .expect("create failed");

        // Creating another volume should fail because there won't be enough space.
        let (_dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        assert_eq!(
            volumes_proxy
                .create(
                    "bar",
                    dir_server_end,
                    CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() },
                    MountOptions::default(),
                )
                .await
                .expect("create failed (FIDL)"),
            Err(zx::sys::ZX_ERR_NO_SPACE)
        );

        volumes_proxy.remove("foo").await.expect("remove failed (FIDL)").expect("remove failed");

        // Creating should now succeed.
        let (_dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volumes_proxy
            .create(
                "bar",
                dir_server_end,
                CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() },
                MountOptions::default(),
            )
            .await
            .expect("create failed (FIDL)")
            .expect("create failed");

        let expected = HashSet::from(["bar".to_string(), "blobfs".to_string(), "data".to_string()]);
        let volumes_dir =
            open_directory(&fixture.outgoing_dir, "volumes", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            &readdir(&volumes_dir)
                .await
                .unwrap()
                .into_iter()
                .map(|d| d.name)
                .collect::<HashSet<_>>(),
            &expected,
        );

        // Check again after a remount.
        let fixture = Fixture::from_fake_server(fixture.take_fake_server()).await;
        let volumes_dir =
            open_directory(&fixture.outgoing_dir, "volumes", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            &readdir(&volumes_dir)
                .await
                .unwrap()
                .into_iter()
                .map(|d| d.name)
                .collect::<HashSet<_>>(),
            &expected,
        );
    }

    #[fuchsia::test]
    async fn test_create_mount_remove() {
        let fixture = Fixture::new(SLICE_SIZE).await;

        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

        {
            let (_dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volumes_proxy
                .create(
                    "foo",
                    dir_server_end,
                    CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() },
                    MountOptions::default(),
                )
                .await
                .expect("create failed (FIDL)")
                .expect("create failed");

            volumes_proxy
                .remove("foo")
                .await
                .expect("remove failed (FIDL)")
                .expect("remove failed");
        }

        let expected = HashSet::from(["blobfs".to_string(), "data".to_string()]);
        let volumes_dir =
            open_directory(&fixture.outgoing_dir, "volumes", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            &readdir(&volumes_dir)
                .await
                .unwrap()
                .into_iter()
                .map(|d| d.name)
                .collect::<HashSet<_>>(),
            &expected,
        );

        // Check again after a remount.
        let fixture = Fixture::from_fake_server(fixture.take_fake_server()).await;
        let volumes_dir =
            open_directory(&fixture.outgoing_dir, "volumes", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            &readdir(&volumes_dir)
                .await
                .unwrap()
                .into_iter()
                .map(|d| d.name)
                .collect::<HashSet<_>>(),
            &expected,
        );
    }

    #[fuchsia::test]
    async fn test_get_volume_info() {
        let fixture = Fixture::new(0).await;

        // Mount the minfs partition.
        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/data",
        )
        .unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let volume_proxy =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();
        let (status, manager_info, volume_info) = volume_proxy.get_volume_info().await.unwrap();

        assert_eq!(status, zx::sys::ZX_OK);
        assert_matches!(
            manager_info.as_deref(),
            Some(fblock::VolumeManagerInfo {
                slice_size: SLICE_SIZE,
                max_virtual_slice: MAX_SLICE_COUNT,
                assigned_slice_count,
                ..
            }) if *assigned_slice_count > 0
        );
        assert_matches!(
            volume_info.as_deref(),
            Some(fblock::VolumeInfo { partition_slice_count: 257, .. })
        );
        // Make sure assigned_slice_count matches block_count from GetInfo.
        let info = volume_proxy.get_info().await.unwrap().expect("get_info failed");
        assert_eq!(
            info.block_count,
            volume_info.unwrap().partition_slice_count * SLICE_SIZE / BLOCK_SIZE as u64
        );
    }

    #[fuchsia::test]
    async fn test_query_slices() {
        let fixture = Fixture::new(0).await;

        // Mount the blobfs partition.
        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/blobfs",
        )
        .unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let volume_proxy =
            Arc::new(connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap());

        let check = |start, allocated, count| {
            let volume_proxy = volume_proxy.clone();
            async move {
                let (status, ranges, range_count) =
                    volume_proxy.query_slices(&[start]).await.unwrap();
                assert_eq!(status, zx::sys::ZX_OK);
                assert_eq!(range_count, 1);
                assert_eq!(ranges[0], fblock::VsliceRange { allocated, count });
                range_count
            }
        };

        let mut slice = 0;
        loop {
            let (status, ranges, _range_count) = volume_proxy.query_slices(&[slice]).await.unwrap();
            if status != 0 {
                break;
            }
            println!("{slice}: {:?}", ranges[0]);
            slice += ranges[0].count;
        }

        check(0, true, 1).await;
        check(1, false, 0x4000 - 1).await;
        check(2, false, 0x4000 - 2).await;
        check(0x4000, true, 1).await;
        check(0x8000, true, 20).await;
        check(0x8001, true, 19).await;
        check(0x8000 + 20, false, 0x4000 - 20).await;
        check(0xc000, true, 17).await;
        check(0xc000 + 17, false, 0x4000 - 17).await;
        check(0x10000, true, 1).await;
        check(0x10001, false, MAX_SLICE_COUNT - 0x10001).await;
    }

    #[fuchsia::test]
    async fn test_extend_bounds() {
        let fixture = Fixture::new(SLICE_SIZE * 3).await;

        // Mount the blobfs partition.
        let volume_proxy = fixture.mount_volume("blobfs").await;

        // No op succeeds.
        assert_eq!(volume_proxy.extend(1, 0).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);
        assert_eq!(volume_proxy.extend(1, 0).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);

        // Trying to do things with slice zero fails.
        assert_eq!(
            volume_proxy.extend(0, 0).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );
        assert_eq!(
            volume_proxy.extend(0, 4).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );

        // Calls are protected against integer overflow.
        assert_eq!(
            volume_proxy.extend(1, u64::MAX).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );
        assert_eq!(
            volume_proxy.extend(u64::MAX, 1).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );

        // Also against numbers we have no hope of addressing to.
        assert_eq!(
            volume_proxy.extend(u32::MAX as u64, 1).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );
        assert_eq!(
            volume_proxy.extend(1, u32::MAX as u64).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );
    }

    #[fuchsia::test]
    async fn test_extend() {
        let fixture = Fixture::new(SLICE_SIZE * 3).await;

        // Mount the blobfs partition.
        let volume_proxy = fixture.mount_volume("blobfs").await;

        assert_eq!(volume_proxy.extend(1, 1).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);

        let client = RemoteBlockClient::new(&volume_proxy).await.unwrap();

        // A write and read spanning the first two slices should now succeed.
        let buf = vec![0xef; 16384];
        let offset = SLICE_SIZE - 8192;
        client.write_at(BufferSlice::Memory(&buf), offset).await.unwrap();
        let mut read_buf = vec![0; 16384];
        client.read_at(MutableBufferSlice::Memory(&mut read_buf), offset).await.unwrap();
        assert_eq!(&buf, &read_buf);

        // Check that query_slices shows the allocation.
        let (status, ranges, range_count) = volume_proxy.query_slices(&[0]).await.unwrap();
        assert_eq!(status, zx::sys::ZX_OK);
        assert_eq!(range_count, 1);
        assert_eq!(ranges[0], fblock::VsliceRange { allocated: true, count: 2 });

        // Try again, and it should fail.
        assert_eq!(
            volume_proxy.extend(1, 1).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_INVALID_ARGS
        );

        // Same, but with overlapping ranges.
        assert_eq!(
            volume_proxy.extend(0x4000 - 2, 4).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_INVALID_ARGS
        );
        assert_eq!(
            volume_proxy.extend(0x8005, 20).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_INVALID_ARGS
        );

        // Extend the minfs partition, so that the extension of the blobfs volume is not contiguous.
        let data_volume_proxy = fixture.mount_volume("data").await;

        assert_eq!(
            data_volume_proxy.extend(0xa0000, 1).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_OK
        );

        // Extend the blobfs partition again.
        assert_eq!(volume_proxy.extend(2, 1).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);

        // Write to the blobfs partition over the two slices we have extended by.
        let data = vec![0x73; SLICE_SIZE as usize * 2];
        client.write_at(BufferSlice::Memory(&data), SLICE_SIZE).await.unwrap();

        // Now read it back.
        let mut buf = vec![0; SLICE_SIZE as usize * 2];
        client.read_at(MutableBufferSlice::Memory(&mut buf), SLICE_SIZE).await.unwrap();

        assert_eq!(&buf, &data);

        // Check we get the same when we remount.
        let fixture = Fixture::from_fake_server(fixture.take_fake_server()).await;
        let client = RemoteBlockClient::new(&fixture.mount_volume("blobfs").await).await.unwrap();

        buf.fill(0);
        client.read_at(MutableBufferSlice::Memory(&mut buf), SLICE_SIZE).await.unwrap();

        assert_eq!(&buf, &data);
    }

    #[fuchsia::test]
    async fn test_shrink_bounds() {
        let fixture = Fixture::new(SLICE_SIZE * 3).await;

        // Mount the blobfs partition.
        let volume_proxy = fixture.mount_volume("blobfs").await;

        // No op succeeds.
        assert_eq!(volume_proxy.shrink(1, 0).await.expect("shrink failed (FIDL)"), zx::sys::ZX_OK);
        assert_eq!(volume_proxy.shrink(1, 0).await.expect("shrink failed (FIDL)"), zx::sys::ZX_OK);

        // Trying to do things with slice zero fails.
        assert_eq!(
            volume_proxy.shrink(0, 0).await.expect("shrink failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );
        assert_eq!(
            volume_proxy.shrink(0, 4).await.expect("shrink failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );

        // Calls are protected against integer overflow.
        assert_eq!(
            volume_proxy.shrink(1, u64::MAX).await.expect("shrink failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );
        assert_eq!(
            volume_proxy.shrink(u64::MAX, 1).await.expect("shrink failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );

        // Also against numbers we have no hope of addressing to.
        assert_eq!(
            volume_proxy.shrink(u32::MAX as u64, 1).await.expect("shrink failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );
        assert_eq!(
            volume_proxy.shrink(1, u32::MAX as u64).await.expect("shrink failed (FIDL)"),
            zx::sys::ZX_ERR_OUT_OF_RANGE
        );
    }

    #[fuchsia::test]
    async fn test_shrink() {
        let final_checks;

        let initial_counts;

        let fake_server = {
            let fixture = Fixture::new(23 * SLICE_SIZE).await;

            let volumes_proxy =
                connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

            // Mount the blobfs partition.
            let blobfs_volume = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
                &fixture.outgoing_dir,
                "volumes/blobfs",
            )
            .unwrap();

            let (dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            blobfs_volume
                .mount(dir_server_end, MountOptions::default())
                .await
                .expect("mount failed (FIDL)")
                .expect("mount failed");

            let blobfs_volume_proxy =
                connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();

            // Record the initial assigned slice count via the blobfs_volume.
            let blobfs_initial_counts = get_counts(&blobfs_volume_proxy).await;

            // Create a new volume.

            let (dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            volumes_proxy
                .create(
                    "foo",
                    dir_server_end,
                    CreateOptions {
                        initial_size: Some(SLICE_SIZE * 5),
                        type_guid: Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
                        ..CreateOptions::default()
                    },
                    MountOptions::default(),
                )
                .await
                .expect("create failed (FIDL)")
                .expect("create failed");

            let volume_proxy =
                connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();

            let get_counts = || get_counts(&volume_proxy);
            initial_counts = get_counts().await;
            assert_eq!(initial_counts, (blobfs_initial_counts.0 + 5, 5));

            // Extend the blobfs volume so we get some fragmentation
            assert_eq!(
                blobfs_volume_proxy.extend(1, 1).await.expect("extend failed (FIDL)"),
                zx::sys::ZX_OK
            );

            assert_eq!(get_counts().await, (initial_counts.0 + 1, 5));

            // Extend the volume we created by another 5 slices.
            assert_eq!(
                volume_proxy.extend(5, 5).await.expect("extend failed (FIDL)"),
                zx::sys::ZX_OK
            );

            assert_eq!(get_counts().await, (initial_counts.0 + 6, 10));

            // And again...
            assert_eq!(
                blobfs_volume_proxy.extend(2, 1).await.expect("extend failed (FIDL)"),
                zx::sys::ZX_OK
            );
            assert_eq!(
                volume_proxy.extend(10, 5).await.expect("extend failed (FIDL)"),
                zx::sys::ZX_OK
            );

            // And again...
            assert_eq!(
                blobfs_volume_proxy.extend(3, 1).await.expect("extend failed (FIDL)"),
                zx::sys::ZX_OK
            );
            assert_eq!(
                volume_proxy.extend(15, 5).await.expect("extend failed (FIDL)"),
                zx::sys::ZX_OK
            );

            assert_eq!(get_counts().await, (initial_counts.0 + 18, 20));

            // Write to every slice.
            let client = RemoteBlockClient::new(&volume_proxy).await.unwrap();
            for i in 0..20 {
                let buf = vec![i; SLICE_SIZE as usize];
                client.write_at(BufferSlice::Memory(&buf), i as u64 * SLICE_SIZE).await.unwrap();
            }

            // Shrink, and check with QuerySlices.
            assert_eq!(
                volume_proxy.shrink(4, 7).await.expect("shrink failed (FIDL)"),
                zx::sys::ZX_OK
            );

            assert_eq!(get_counts().await, (initial_counts.0 + 11, 13));

            let (status, ranges, range_count) =
                volume_proxy.query_slices(&[0, 4, 11]).await.unwrap();
            assert_eq!(status, zx::sys::ZX_OK);
            assert_eq!(range_count, 3);
            assert_eq!(
                &ranges[..3],
                &[
                    fblock::VsliceRange { allocated: true, count: 4 },
                    fblock::VsliceRange { allocated: false, count: 7 },
                    fblock::VsliceRange { allocated: true, count: 9 }
                ]
            );

            // Delete the last range we added, which should occupy a whole mapping.
            assert_eq!(
                volume_proxy.shrink(15, 5).await.expect("shrink failed (FIDL)"),
                zx::sys::ZX_OK
            );

            assert_eq!(get_counts().await, (initial_counts.0 + 6, 8));

            let (status, ranges, range_count) =
                volume_proxy.query_slices(&[0, 4, 11, 15]).await.unwrap();
            assert_eq!(status, zx::sys::ZX_OK);
            assert_eq!(range_count, 4);
            assert_eq!(
                &ranges[..4],
                &[
                    fblock::VsliceRange { allocated: true, count: 4 },
                    fblock::VsliceRange { allocated: false, count: 7 },
                    fblock::VsliceRange { allocated: true, count: 4 },
                    fblock::VsliceRange { allocated: false, count: MAX_SLICE_COUNT - 15 }
                ]
            );

            // Delete a chunk within a single mapping.
            assert_eq!(
                volume_proxy.shrink(1, 2).await.expect("shrink failed (FIDL)"),
                zx::sys::ZX_OK
            );

            assert_eq!(get_counts().await, (initial_counts.0 + 4, 6));

            // Some checks that we also want to perform after reopening.
            final_checks = |volume_proxy: fblock::BlockProxy| async move {
                let (status, ranges, range_count) =
                    volume_proxy.query_slices(&[0, 1, 3, 4, 11, 15]).await.unwrap();
                assert_eq!(status, zx::sys::ZX_OK);
                assert_eq!(range_count, 6);
                assert_eq!(
                    &ranges[..6],
                    &[
                        fblock::VsliceRange { allocated: true, count: 1 },
                        fblock::VsliceRange { allocated: false, count: 2 },
                        fblock::VsliceRange { allocated: true, count: 1 },
                        fblock::VsliceRange { allocated: false, count: 7 },
                        fblock::VsliceRange { allocated: true, count: 4 },
                        fblock::VsliceRange { allocated: false, count: MAX_SLICE_COUNT - 15 }
                    ]
                );

                // Read back and check all slices.
                let client = RemoteBlockClient::new(&volume_proxy).await.unwrap();
                for i in [0, 3, 11, 12, 13, 14] {
                    let mut buf = vec![0; SLICE_SIZE as usize];
                    client
                        .read_at(MutableBufferSlice::Memory(&mut buf), i as u64 * SLICE_SIZE)
                        .await
                        .unwrap();
                    assert_eq!(&buf, &vec![i; SLICE_SIZE as usize]);
                }
            };

            final_checks(volume_proxy).await;

            fixture.fake_server
        };

        // Reopen, and check we get the same.
        let fixture = Fixture::from_fake_server(fake_server).await;

        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/foo",
        )
        .unwrap();
        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let volume_proxy =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();

        assert_eq!(get_counts(&volume_proxy).await, (initial_counts.0 + 4, 6));

        final_checks(volume_proxy).await;
    }

    #[fuchsia::test]
    async fn test_shrink_with_gaps() {
        let fixture = Fixture::new(SLICE_SIZE * 23).await;

        // Mount the blobfs partition.
        let _blobfs_volume_proxy = fixture.mount_volume("blobfs").await;

        // Make a new volume to play with.
        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();
        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volumes_proxy
            .create(
                "foo",
                dir_server_end,
                CreateOptions {
                    initial_size: Some(SLICE_SIZE * 5),
                    type_guid: Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
                    ..CreateOptions::default()
                },
                MountOptions::default(),
            )
            .await
            .expect("create failed (FIDL)")
            .expect("create failed");

        let volume_proxy =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();

        let get_counts = || get_counts(&volume_proxy);
        let initial_counts = get_counts().await;

        assert_eq!(volume_proxy.extend(5, 5).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);
        assert_eq!(get_counts().await, (initial_counts.0 + 5, 10));

        // Shrink past the end succeeds
        assert_eq!(volume_proxy.shrink(5, 8).await.expect("shrink failed (FIDL)"), zx::sys::ZX_OK);
        assert_eq!(get_counts().await, (initial_counts.0, 5));

        assert_eq!(volume_proxy.extend(5, 5).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);
        assert_eq!(get_counts().await, (initial_counts.0 + 5, 10));
        assert_eq!(volume_proxy.extend(15, 5).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);
        assert_eq!(get_counts().await, (initial_counts.0 + 10, 15));

        // Shrink across a gap succeeds
        assert_eq!(volume_proxy.shrink(5, 12).await.expect("shrink failed (FIDL)"), zx::sys::ZX_OK);
        assert_eq!(get_counts().await, (initial_counts.0 + 3, 8));
        // We can shrink the rest we left at the end
        assert_eq!(volume_proxy.shrink(15, 5).await.expect("shrink failed (FIDL)"), zx::sys::ZX_OK);
        assert_eq!(get_counts().await, (initial_counts.0, 5));
    }

    #[fuchsia::test]
    async fn test_limits() {
        let fixture = Fixture::new(10 * SLICE_SIZE).await;

        // Mount the blobfs partition.
        let blobfs_volume = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/blobfs",
        )
        .unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        blobfs_volume
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let volume_proxy =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();

        let (status, manager_info, volume_info) = volume_proxy.get_volume_info().await.unwrap();
        assert_eq!(status, zx::sys::ZX_OK);

        // Set the limit for Blobfs to be just one more slice.
        let limit =
            (volume_info.unwrap().partition_slice_count + 1) * manager_info.unwrap().slice_size;
        blobfs_volume
            .set_limit(limit)
            .await
            .expect("set_limit FIDL failed")
            .expect("set_limit failed");

        assert_eq!(
            blobfs_volume
                .get_limit()
                .await
                .expect("get_limit FIDL failed")
                .expect("get_limmit_failed"),
            limit
        );

        // Try and extend by two slices and it should fail with out-of-space.
        assert_eq!(
            volume_proxy.extend(1, 2).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_NO_SPACE
        );

        // But one should be fine.
        assert_eq!(volume_proxy.extend(1, 1).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);

        // And then another one should fail.
        assert_eq!(
            volume_proxy.extend(2, 1).await.expect("extend failed (FIDL)"),
            zx::sys::ZX_ERR_NO_SPACE
        );
    }

    #[fuchsia::test]
    async fn test_force_access_passed_through() {
        let expect_force_access = Arc::new(AtomicBool::new(false));

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

        let contents = std::fs::read("/pkg/data/golden-fvm.blk").unwrap();
        let fake_server = Arc::new(
            VmoBackedServerOptions {
                block_size: BLOCK_SIZE,
                initial_contents: InitialContents::FromBuffer(&contents),
                observer: Some(Box::new(ForceAccessObserver(expect_force_access.clone()))),
                info: DeviceInfo::Block(BlockInfo {
                    device_flags: fblock::DeviceFlag::FUA_SUPPORT,
                    ..Default::default()
                }),
                ..Default::default()
            }
            .build()
            .unwrap(),
        );

        let fixture = Fixture::from_fake_server(fake_server).await;

        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/data",
        )
        .unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let client = RemoteBlockClient::new(
            &connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap(),
        )
        .await
        .unwrap();

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
    }

    #[fuchsia::test]
    async fn test_flush_after_metadata_write() {
        let flush_called = Arc::new(AtomicU64::new(0));

        struct FlushObserver(Arc<AtomicU64>);

        impl Observer for FlushObserver {
            fn flush(&self, _writes: Option<&mut WriteCache>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let contents = std::fs::read("/pkg/data/golden-fvm.blk").unwrap();
        let fake_server = Arc::new(
            VmoBackedServerOptions {
                block_size: BLOCK_SIZE,
                initial_contents: InitialContents::FromCapacityAndBuffer(
                    (contents.len() as u64 + SLICE_SIZE) / BLOCK_SIZE as u64,
                    &contents,
                ),
                observer: Some(Box::new(FlushObserver(flush_called.clone()))),
                ..Default::default()
            }
            .build()
            .unwrap(),
        );

        let fixture = Fixture::from_fake_server(fake_server).await;

        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

        // Because we added an extra slice worth of space to the disk, initialization writes the
        // metadata once to update the maximum number of physical slices available.
        assert_eq!(flush_called.load(Ordering::Relaxed), 1);

        let (_dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volumes_proxy
            .create(
                "foo",
                dir_server_end,
                CreateOptions {
                    type_guid: Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
                    ..CreateOptions::default()
                },
                MountOptions::default(),
            )
            .await
            .expect("create failed (FIDL)")
            .expect("create failed");

        assert_eq!(flush_called.load(Ordering::Relaxed), 2);
    }

    #[fuchsia::test]
    async fn ensure_allocated() {
        let fixture = Fixture::new(10 * SLICE_SIZE).await;
        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();
        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();

        volumes_proxy
            .create(
                "foo",
                dir_server_end,
                CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() },
                MountOptions::default(),
            )
            .await
            .expect("create failed (FIDL)")
            .expect("create failed");

        let volume_proxy =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();
        let (original_manager_slice_count, _) = get_counts(&volume_proxy).await;

        assert_eq!(volume_proxy.extend(3, 1).await.expect("extend failed (FIDL)"), zx::sys::ZX_OK);

        let fvm = fixture.component.fvm();
        let partition_index = fvm.find_partition_with_name("foo").await.unwrap();
        fvm.ensure_allocated(partition_index, 5).await.unwrap();

        let (status, ranges, range_count) = volume_proxy.query_slices(&[0, 5]).await.unwrap();
        assert_eq!(status, zx::sys::ZX_OK);
        assert_eq!(range_count, 2);
        assert_eq!(
            &ranges[..2],
            &[
                fblock::VsliceRange { allocated: true, count: 5 },
                fblock::VsliceRange { allocated: false, count: MAX_SLICE_COUNT - 5 }
            ]
        );

        let (manager_slice_count, partition_slice_count) = get_counts(&volume_proxy).await;
        // When the volume was created, it uses 1 slice.
        assert_eq!(manager_slice_count, original_manager_slice_count + 4);
        assert_eq!(partition_slice_count, 5);

        // Write to the assigned slices and then read back.
        let client = RemoteBlockClient::new(&volume_proxy).await.unwrap();
        let mut data = vec![0; SLICE_SIZE as usize * 5];
        for (i, chunk) in data.chunks_exact_mut(BLOCK_SIZE as usize).enumerate() {
            chunk.fill(i as u8);
        }

        client.write_at(BufferSlice::Memory(&data), 0).await.unwrap();

        let mut read = vec![0; SLICE_SIZE as usize * 5];
        client.read_at(MutableBufferSlice::Memory(&mut read), 0).await.unwrap();

        assert_eq!(&read, &data);
    }

    #[fuchsia::test(threads = 2)]
    async fn test_unmount_volume_race() {
        let fixture = Fixture::new(SLICE_SIZE).await;

        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

        for _ in 0..100 {
            let (dir_proxy, dir_server_end) =
                fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            let volumes_proxy = volumes_proxy.clone();
            volumes_proxy
                .create(
                    "foo",
                    dir_server_end,
                    CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() },
                    MountOptions::default(),
                )
                .await
                .expect("create failed (FIDL)")
                .expect("create failed");

            // Drop the volume in one thread, remove it explicitly in the other.  Neither branch
            // should ever fail, regardless of who wins the race.
            futures::join! {
                async move {
                    std::mem::drop(dir_proxy);
                },
                async move {
                    volumes_proxy
                        .remove("foo").await.expect("remove failed (FIDL)").expect("remove failed");
                },
            };

            let expected = HashSet::from(["blobfs".to_string(), "data".to_string()]);
            let volumes_dir =
                open_directory(&fixture.outgoing_dir, "volumes", fio::PERM_READABLE).await.unwrap();
            assert_eq!(
                &readdir(&volumes_dir)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|d| d.name)
                    .collect::<HashSet<_>>(),
                &expected,
            );
        }
    }

    #[fuchsia::test]
    async fn test_admin_shutdown_no_volumes() {
        let fixture = Fixture::new(SLICE_SIZE).await;
        let admin_proxy =
            connect_to_protocol_at_dir_svc::<AdminMarker>(&fixture.outgoing_dir).unwrap();
        admin_proxy.shutdown().await.unwrap();
        fixture.component.scope.wait().await;
    }

    #[fuchsia::test]
    async fn test_admin_shutdown_with_volumes() {
        let fixture = Fixture::new(SLICE_SIZE).await;

        let (_dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();
        volumes_proxy
            .create(
                "foo",
                dir_server_end,
                CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() },
                MountOptions::default(),
            )
            .await
            .expect("create failed (FIDL)")
            .expect("create failed");

        let admin_proxy =
            connect_to_protocol_at_dir_svc::<AdminMarker>(&fixture.outgoing_dir).unwrap();
        admin_proxy.shutdown().await.unwrap();
        fixture.component.scope.wait().await;
    }

    #[fuchsia::test]
    async fn test_trim() {
        let trim_called = Arc::new(AtomicBool::new(false));

        struct TrimObserver(Arc<AtomicBool>);

        impl Observer for TrimObserver {
            fn trim(&self, _device_block_offset: u64, _block_count: u32) {
                self.0.store(true, Ordering::Relaxed)
            }
        }

        let contents = std::fs::read("/pkg/data/golden-fvm.blk").unwrap();
        let fake_server = Arc::new(
            VmoBackedServerOptions {
                block_size: BLOCK_SIZE,
                initial_contents: InitialContents::FromBuffer(&contents),
                observer: Some(Box::new(TrimObserver(trim_called.clone()))),
                ..Default::default()
            }
            .build()
            .unwrap(),
        );

        let fixture = Fixture::from_fake_server(fake_server).await;

        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/data",
        )
        .unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let client = RemoteBlockClient::new(
            &connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap(),
        )
        .await
        .unwrap();

        client.trim(0..8192).await.unwrap();

        assert!(trim_called.load(Ordering::Relaxed));
    }

    #[fuchsia::test]
    async fn test_volume_info() {
        let contents = std::fs::read("/pkg/data/golden-fvm.blk").unwrap();
        let fake_server = Arc::new(
            VmoBackedServerOptions {
                info: DeviceInfo::Block(BlockInfo {
                    device_flags: fblock::DeviceFlag::READONLY,
                    ..Default::default()
                }),
                block_size: BLOCK_SIZE,
                initial_contents: InitialContents::FromBuffer(&contents),
                ..Default::default()
            }
            .build()
            .unwrap(),
        );

        let fixture = Fixture::from_fake_server(fake_server).await;

        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/data",
        )
        .unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volume_proxy
            .mount(dir_server_end, MountOptions::default())
            .await
            .expect("mount failed (FIDL)")
            .expect("mount failed");

        let proxy = connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();
        let info = proxy.get_info().await.expect("FIDL error").expect("get_info failed");
        assert_eq!(info.block_count, 16448);
        assert_eq!(info.block_size, 512);
        assert_eq!(
            info.flags,
            fblock::DeviceFlag::READONLY | fblock::DeviceFlag::ZSTD_DECOMPRESSION_SUPPORT
        );

        let partition_metadata =
            proxy.get_metadata().await.expect("FIDL error").expect("get_metadata failed");
        assert_eq!(partition_metadata.name, Some("data".to_string()));
        assert_eq!(partition_metadata.type_guid.unwrap().value, DATA_TYPE_GUID);
        assert_eq!(partition_metadata.instance_guid.unwrap().value, [0u8; 16]);
        assert_eq!(partition_metadata.start_block_offset, None);
        assert_eq!(partition_metadata.num_blocks, None);
        assert_eq!(partition_metadata.flags, Some(0));

        let (status, _, volume_info) = proxy.get_volume_info().await.expect("FIDL error");
        assert_eq!(status, zx::sys::ZX_OK);
        let volume_info = volume_info.unwrap();
        assert_eq!(volume_info.partition_slice_count, 257);
        assert_eq!(volume_info.slice_limit, 0);
    }

    #[fuchsia::test]
    async fn test_fsck_golden_fvm() {
        let contents = std::fs::read("/pkg/data/golden-fvm.blk").unwrap();
        let fake_server = Arc::new(
            VmoBackedServerOptions {
                info: DeviceInfo::Block(BlockInfo {
                    device_flags: fblock::DeviceFlag::READONLY,
                    ..Default::default()
                }),
                block_size: BLOCK_SIZE,
                initial_contents: InitialContents::FromBuffer(&contents),
                ..Default::default()
            }
            .build()
            .unwrap(),
        );

        let fixture = Fixture::from_fake_server(fake_server).await;

        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/blobfs",
        )
        .unwrap();

        volume_proxy
            .check(CheckOptions { uri: Some("#meta/blobfs.cm".to_string()), ..Default::default() })
            .await
            .expect("check blob failed (FIDL)")
            .expect("check blob failed");
    }

    #[fuchsia::test]
    async fn test_get_info() {
        let fixture = Fixture::new(SLICE_SIZE).await;

        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

        let type_guid = [1u8; 16];
        let guid = [2u8; 16];
        let (_volume_dir_proxy, volume_dir_server_end) =
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volumes_proxy
            .create(
                "test_vol",
                volume_dir_server_end,
                CreateOptions {
                    initial_size: Some(SLICE_SIZE),
                    type_guid: Some(type_guid),
                    guid: Some(guid),
                    ..Default::default()
                },
                MountOptions::default(),
            )
            .await
            .expect("create failed (FIDL)")
            .expect("create failed");

        let volume_proxy = connect_to_named_protocol_at_dir_root::<VolumeMarker>(
            &fixture.outgoing_dir,
            "volumes/test_vol",
        )
        .unwrap();

        let info = volume_proxy
            .get_info()
            .await
            .expect("get_info failed (FIDL)")
            .expect("get_info failed");
        assert_eq!(info.guid, Some(guid));
    }

    #[fuchsia::test]
    async fn test_volume_manager_info_golden_fvm_disk_size_change() {
        let contents = std::fs::read("/pkg/data/golden-fvm.blk").unwrap();

        let info_small_disk = {
            let fake_server = Arc::new(
                VmoBackedServerOptions {
                    block_size: BLOCK_SIZE,
                    initial_contents: InitialContents::FromBuffer(&contents),
                    ..Default::default()
                }
                .build()
                .unwrap(),
            );

            let fixture = Fixture::from_fake_server(fake_server).await;

            let volumes_proxy =
                connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

            volumes_proxy
                .get_info()
                .await
                .expect("get info failed (FIDL)")
                .expect("get info failed")
                .expect("get info returned no info")
        };

        let info_larger_disk = {
            let fake_server = Arc::new(
                VmoBackedServerOptions {
                    block_size: BLOCK_SIZE,
                    initial_contents: InitialContents::FromCapacityAndBuffer(
                        (contents.len() as u64 + SLICE_SIZE) / BLOCK_SIZE as u64,
                        &contents,
                    ),
                    ..Default::default()
                }
                .build()
                .unwrap(),
            );

            let fixture = Fixture::from_fake_server(fake_server).await;

            let volumes_proxy =
                connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

            volumes_proxy
                .get_info()
                .await
                .expect("get info failed (FIDL)")
                .expect("get info failed")
                .expect("get info returned no info")
        };

        // The disk has space for one more slice so it reports that now.
        assert_eq!(info_small_disk.slice_count + 1, info_larger_disk.slice_count);

        // All the other info should be the same.
        assert_eq!(info_small_disk.slice_size, info_larger_disk.slice_size);
        assert_eq!(info_small_disk.assigned_slice_count, info_larger_disk.assigned_slice_count);
        assert_eq!(info_small_disk.maximum_slice_count, info_larger_disk.maximum_slice_count);
        assert_eq!(info_small_disk.max_virtual_slice, info_larger_disk.max_virtual_slice);

        // We also sanity check some of the values while trying not to be a change detector. The
        // golden image starts with all it's slices allocated to two partitions, so before changing
        // size it should be equal, and afterward it should be off by one.
        assert_eq!(info_small_disk.assigned_slice_count, info_small_disk.slice_count);
        assert_eq!(info_larger_disk.assigned_slice_count + 1, info_larger_disk.slice_count);

        // Also, slice_count <= maximum_slice_count <= max_virtual_slice == MAX_SLICE_COUNT
        assert!(info_small_disk.slice_count <= info_small_disk.maximum_slice_count);
        assert!(info_small_disk.maximum_slice_count <= info_small_disk.max_virtual_slice);
        assert_eq!(info_small_disk.max_virtual_slice, MAX_SLICE_COUNT);
    }
    struct MockCrypt;

    impl MockCrypt {
        fn new() -> fidl::endpoints::ClientEnd<CryptMarker> {
            let (client, mut stream) = fidl::endpoints::create_request_stream::<CryptMarker>();
            fasync::Task::spawn(async move {
                while let Some(request) = stream.try_next().await.unwrap() {
                    match request {
                        CryptRequest::CreateKey { responder, .. } => {
                            // ZxcryptHeaderAndKey size is 16 + 16 + 4 + 96 = 132
                            let mut wrapped = vec![0; 132];
                            // Magic
                            wrapped[0..16].copy_from_slice(&ZXCRYPT_MAGIC);
                            // Guid (offset 16, 16 bytes) - leave as 0
                            // Version (offset 32, 4 bytes)
                            wrapped[32..36].copy_from_slice(&0x01000000_u32.to_le_bytes());

                            let unwrapped = vec![0; 80];
                            // Return status 0, wrapped key, unwrapped key
                            responder.send(Ok((&[0u8; 16], &wrapped, &unwrapped))).unwrap();
                        }
                        CryptRequest::UnwrapKey { responder, .. } => {
                            let unwrapped = vec![0; 80];
                            responder.send(Ok(&unwrapped)).unwrap();
                        }
                        _ => panic!("Unexpected request"),
                    }
                }
            })
            .detach();
            client
        }
    }

    #[fuchsia::test]
    async fn test_fragmented_encrypted_io() {
        // Regression test for https://fxbug.dev/478029520.
        let fixture = Fixture::new(100 * SLICE_SIZE).await;

        let volumes_proxy =
            connect_to_protocol_at_dir_svc::<VolumesMarker>(&fixture.outgoing_dir).unwrap();

        let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let crypt_client = MockCrypt::new();

        volumes_proxy
            .create(
                "target",
                dir_server_end,
                CreateOptions { type_guid: Some([1; 16]), ..CreateOptions::default() },
                MountOptions { crypt: Some(crypt_client), ..MountOptions::default() },
            )
            .await
            .expect("create failed (FIDL)")
            .expect("create failed");

        let target_block =
            connect_to_protocol_at_dir_svc::<fblock::BlockMarker>(&dir_proxy).unwrap();

        // Extend target by 31 slices, bringing its total to 32.
        assert_eq!(target_block.extend(1, 31).await.expect("extend FIDL"), zx::sys::ZX_OK);

        // Create other volume to occupy next physical slice.
        let (_dir_proxy, dir_server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        volumes_proxy
            .create(
                "other",
                dir_server_end,
                CreateOptions { type_guid: Some([2; 16]), ..CreateOptions::default() },
                MountOptions::default(),
            )
            .await
            .unwrap()
            .unwrap();

        // Extend target by 1 more slice.
        assert_eq!(target_block.extend(32, 1).await.expect("extend FIDL"), zx::sys::ZX_OK);

        let client = RemoteBlockClient::new(target_block).await.unwrap();
        let vmo = zx::Vmo::create(33 * SLICE_SIZE as u64).unwrap();
        // SAFETY: Test code, only attach once, no other mappings.
        let vmo_id = unsafe { client.attach_vmo(&vmo) }.await.unwrap();

        // Check both reads and writes.
        // Due to a logic bug, reads and writes returned ZX_ERR_INVALID_ARGS, which this is a
        // regression test for.
        client
            .read_at(
                MutableBufferSlice::VmoId {
                    vmo_id: &vmo_id,
                    offset: 0,
                    length: 33 * SLICE_SIZE as u64,
                },
                0,
            )
            .await
            .expect("read failed");
        client
            .write_at(
                BufferSlice::VmoId { vmo_id: &vmo_id, offset: 0, length: 33 * SLICE_SIZE as u64 },
                0,
            )
            .await
            .expect("write failed");
        client.detach_vmo(vmo_id).await.unwrap();
    }

    #[fuchsia::test]
    async fn test_invalid_allocation_table_size() {
        use zerocopy::IntoBytes as _;
        let header = HeaderRaw {
            magic: MAGIC,
            major_version: MAJOR_VERSION,
            pslice_count: 10,
            slice_size: SLICE_SIZE,
            fvm_partition_size: 10 * BLOCK_SIZE as u64,
            vpartition_table_size: MAX_PARTITIONS * std::mem::size_of::<PartitionEntry>() as u64,
            allocation_table_size: u64::MAX,
            generation: 0,
            hash: [0; 32],
            oldest_minor_version: MINOR_VERSION,
        };
        let header_bytes = header.as_bytes();

        let block_size = 512;
        let vmo_size = 128 * 1024;
        let vmo = zx::Vmo::create(vmo_size).unwrap();
        vmo.write(header_bytes, 0).unwrap();

        let (client_end, server_end) = fidl::endpoints::create_endpoints::<BlockMarker>();
        let fake_server = Arc::new(
            VmoBackedServer::from_vmo(block_size, vmo).expect("Failed to create VmoBackedServer"),
        );
        let _task = fasync::Task::spawn(async move {
            let _ = fake_server.serve(server_end.into_stream()).await;
        });
        let block_proxy = client_end.into_proxy();
        let block_client = RemoteBlockClient::new(block_proxy).await.unwrap();

        let mut header_block = vec![0; BLOCK_SIZE as usize];
        block_client.read_at(MutableBufferSlice::Memory(&mut header_block), 0).await.unwrap();

        let result = Metadata::read(&header_block, &block_client, 0).await;
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.err().unwrap());
        assert!(
            err_msg.contains("Bad allocation table size"),
            "Expected 'Bad allocation table size', got '{}'",
            err_msg
        );
    }
}
