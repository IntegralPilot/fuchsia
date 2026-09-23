// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod format;
pub mod metadata;

use crate::metadata::{SuperDeviceRange, SuperMetadata, round_up_to_alignment};
use anyhow::{Error, anyhow, bail, ensure};
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;
use storage_device::buffer::{BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::BufferFuture;
use storage_device::{Device, ReadOptions, WriteOptions};

/// Struct to help interpret the deserialized "super" image.
pub struct SuperParser {
    super_metadata: SuperMetadata,
    metadata_used_regions: BTreeSet<SuperDeviceRange>,
    device: Arc<dyn Device>,
}

impl SuperParser {
    pub async fn new(device: Arc<dyn Device>) -> Result<Self, Error> {
        let super_metadata = SuperMetadata::load_from_device(device.as_ref()).await?;

        let mut metadata_used_regions = BTreeSet::new();
        metadata_used_regions.insert(SuperDeviceRange(
            format::PARTITION_RESERVED_BYTES as u64..format::RESERVED_AND_GEOMETRIES_SIZE,
        ));

        for (i, metadata_slot) in super_metadata.metadata_slots.iter().enumerate() {
            let size = metadata_slot.header().header_size as u64
                + metadata_slot.header().tables_size as u64;
            let aligned_size = round_up_to_alignment(size, device.block_size() as u64)?;

            let primary_offset = super_metadata.geometry.get_primary_metadata_offset(i as u32)?;
            metadata_used_regions
                .insert(SuperDeviceRange(primary_offset..primary_offset + aligned_size));

            let backup_offset = super_metadata.geometry.get_backup_metadata_offset(i as u32)?;
            metadata_used_regions
                .insert(SuperDeviceRange(backup_offset..backup_offset + aligned_size));
        }

        Ok(Self {
            super_metadata,
            metadata_used_regions: into_merged_regions(metadata_used_regions),
            device: device.clone(),
        })
    }

    pub fn super_metadata(&self) -> &SuperMetadata {
        &self.super_metadata
    }

    /// Returns a vector of the used metadata regions in-order, as a half-open Range(start..end).
    pub fn metadata_used_regions_in_bytes(&self) -> Vec<Range<u64>> {
        self.metadata_used_regions.iter().map(|r| (**r).clone()).collect()
    }

    /// Returns a vector of the used partition regions in-order, as a half-open Range(start..end).
    /// Note that the results would be more meaningful for extents with target type
    /// `TARGET_TYPE_LINEAR` as it implies that the extent is a dm-linear target which are made by
    /// concatenating linear regions (extents) of disk together. For `TARGET_TYPE_ZERO`, this would
    /// return [Range(0..0)].
    pub fn partition_used_regions_in_bytes(&self) -> Result<Vec<Range<u64>>, Error> {
        let mut partition_used_regions = BTreeSet::new();
        for metadata_slot in &self.super_metadata.metadata_slots {
            partition_used_regions.append(&mut metadata_slot.get_all_used_extents_as_byte_range()?);
        }
        Ok(into_merged_regions(partition_used_regions).into_iter().map(|r| r.into()).collect())
    }

    /// Get a partition within super. Must specify the name of the sub-partition and which slot from
    /// super to read from.
    pub fn get_sub_partition(
        &self,
        name: &str,
        slot_index: usize,
    ) -> Result<SuperPartitionDevice, Error> {
        let metadata_slot = &self.super_metadata.metadata_slots[slot_index];
        let extent_locations = metadata_slot.extent_locations_for_partition(name)?;
        assert!(
            self.block_size() % self.device.block_size() == 0,
            "block size must be a multiple of device block size."
        );
        Ok(SuperPartitionDevice::new(self.device.clone(), extent_locations)?)
    }

    fn block_size(&self) -> u32 {
        self.super_metadata.block_size()
    }
}

fn into_merged_regions(mut regions: BTreeSet<SuperDeviceRange>) -> BTreeSet<SuperDeviceRange> {
    let mut merged_used_regions = BTreeSet::new();
    // BTreeSet will pop the regions in order (the ranges are sorted by the start of the range
    // first followed by the end).
    let mut current = regions.pop_first();
    if let Some(current_region) = &mut current {
        while let Some(next_region) = regions.pop_first() {
            if (*next_region).start > (*current_region).end {
                // This region is disjoint and it comes after `current_region`.
                merged_used_regions.insert(current_region.clone());
                *current_region = next_region;
            } else {
                // There is an overlap of regions - the start of this region is within the
                // current region. Update the end if needed.
                if (*next_region).end > (*current_region).end {
                    (*current_region).end = (*next_region).end;
                }
            }
        }
        // Insert the remaining region.
        merged_used_regions.insert(current_region.clone());
    }
    merged_used_regions
}

#[derive(Clone)]
pub struct SuperPartitionDevice {
    device: Arc<dyn Device>,
    // Stores mapping of the (inclusive) end of the logical range to the physical range (which is
    // a half-open bounded range. So for `x` in the range (start..end), start <= x < end).
    extents_map: BTreeMap<u64, SuperDeviceRange>,
    partition_size_in_bytes: u64,
}

impl SuperPartitionDevice {
    pub fn new(
        device: Arc<dyn Device>,
        extent_locations: Vec<SuperDeviceRange>,
    ) -> Result<Self, Error> {
        let mut extent_map = BTreeMap::new();
        let mut cursor = 0;
        for physical_range in &extent_locations {
            let size = physical_range.end - physical_range.start;
            cursor += size;
            // Subtract 1 to get the inclusive end of the logical range.
            extent_map.insert(cursor - 1, physical_range.clone());
        }
        Ok(Self {
            device: device.clone(),
            extents_map: extent_map,
            partition_size_in_bytes: round_up_to_alignment(cursor, device.block_size() as u64)?,
        })
    }

    /// Returns all physical extents for this partition, in logical order.
    pub fn extents(&self) -> Vec<Range<u64>> {
        self.extents_map.values().map(|r| r.start..r.end).collect()
    }

    /// Returns the set of physical extents for a given logical range.
    /// The returned extents are physical byte ranges on the underlying device.
    /// Fails if the range contains sparse gaps or is out of bounds.
    pub fn get_extents_for_range(&self, range: Range<u64>) -> Result<Vec<Range<u64>>, Error> {
        let mut result = Vec::new();
        let mut current_logical = range.start;

        while current_logical < range.end {
            // Find the extent that covers `current_logical`.
            // `extents_map` keys are the *inclusive end* of the logical range.
            // So if we have extents [0, 100], [101, 200], keys are 100, 200.
            let mut range_iter = self.extents_map.range(current_logical..);
            if let Some((&logical_end_inclusive, physical_range)) = range_iter.next() {
                let extent_len = physical_range.end - physical_range.start;
                let logical_start = logical_end_inclusive + 1 - extent_len;

                // If `current_logical` < `logical_start`, it means there is a gap (hole)
                // or we are before the first extent.
                if current_logical < logical_start {
                    // We are in a hole before this extent. We don't support this.
                    bail!("get_extents_for_range() called over a sparse range.");
                }

                let intersection_start = current_logical;
                let intersection_end = std::cmp::min(range.end, logical_end_inclusive + 1);

                if intersection_start < intersection_end {
                    let offset_within_extent = intersection_start - logical_start;
                    let len = intersection_end - intersection_start;
                    let physical_start = physical_range.start + offset_within_extent;
                    result.push(physical_start..physical_start + len);
                    current_logical = intersection_end;
                } else {
                    bail!("bug in get_extents_for_range or extent_maps corrupt");
                }
            } else {
                bail!("get_extents_for_range() tried to read past end of partition.");
            }
        }
        Ok(result)
    }
}

#[async_trait]
impl Device for SuperPartitionDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.device.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.device.block_size() as u32
    }

    fn block_count(&self) -> u64 {
        self.partition_size_in_bytes / self.block_size() as u64
    }

    async fn read_with_opts(
        &self,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        let block_size = self.block_size() as u64;
        ensure!(offset % block_size == 0, "misaligned read at offset");
        ensure!(buffer.len() % block_size as usize == 0, "misaligned read for buffer length");
        if buffer.len() == 0 {
            return Ok(());
        }

        // Read may have to be split across multiple sub-reads across different extents.
        let mut logical_cursor = offset;
        let mut buffer_offset = 0;

        // Find the first extent to read from.
        let mut extents = self.extents_map.range(logical_cursor..);
        let (&logical_range_inclusive_end, mut physical_range) =
            extents.next().ok_or_else(|| anyhow!("Offset {} is out of bounds", offset))?;
        let mut extent_len = physical_range.end - physical_range.start;
        let mut logical_range =
            (logical_range_inclusive_end + 1 - extent_len)..(logical_range_inclusive_end + 1);

        while buffer_offset < buffer.len() {
            // Jump to the next extent if we're past the end of the current one.
            if logical_cursor >= logical_range.end {
                let (next_logical_range_inclusive_end, next_physical_range) = extents
                    .next()
                    .ok_or_else(|| anyhow!("Offset {} is out of bounds", logical_cursor))?;
                physical_range = next_physical_range;
                extent_len = physical_range.end - physical_range.start;
                logical_range = (next_logical_range_inclusive_end + 1 - extent_len)
                    ..(next_logical_range_inclusive_end + 1);
            }

            let physical_cursor = physical_range.start + (logical_cursor - logical_range.start);
            let subbuffer_len = std::cmp::min(
                (logical_range.end - logical_cursor) as usize,
                buffer.len() - buffer_offset,
            );
            ensure!(physical_cursor % block_size == 0, "misaligned read at offset");
            ensure!(subbuffer_len % block_size as usize == 0, "misaligned read for buffer length");
            self.device
                .read(
                    physical_cursor,
                    buffer.reborrow().subslice_mut(buffer_offset..buffer_offset + subbuffer_len),
                )
                .await?;
            buffer_offset += subbuffer_len;
            logical_cursor += subbuffer_len as u64;
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn supports_trim(&self) -> bool {
        false
    }

    async fn write_with_opts(
        &self,
        _offset: u64,
        _buffer: BufferRef<'_>,
        _write_opts: WriteOptions,
    ) -> Result<(), Error> {
        Err(anyhow!("read-only partition"))
    }

    async fn flush(&self) -> Result<(), Error> {
        Err(anyhow!("read-only partition"))
    }

    async fn trim(&self, _range: Range<u64>) -> Result<(), Error> {
        Err(anyhow!("read-only partition"))
    }
}

#[cfg(test)]
mod tests {
    use crate::{SuperDeviceRange, SuperParser, SuperPartitionDevice, into_merged_regions};
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::Arc;
    use storage_device::Device;
    use storage_device::fake_device::FakeDevice;
    use zerocopy::IntoBytes;

    use crate::format::{
        BlockDeviceFlags, METADATA_GEOMETRY_MAGIC, METADATA_HEADER_MAGIC, METADATA_MAJOR_VERSION,
        METADATA_VERSION_FOR_EXPANDED_HEADER_MIN, MetadataBlockDevice, MetadataExtent,
        MetadataGeometry, MetadataHeader, MetadataHeaderFlags, MetadataPartition,
        MetadataPartitionGroup, MetadataTableDescriptor, PARTITION_RESERVED_BYTES,
        PartitionAttributes, PartitionGroupFlags, RESERVED_AND_GEOMETRIES_SIZE, SECTOR_SIZE,
        TARGET_TYPE_LINEAR,
    };
    use sha2::Digest;

    const BLOCK_SIZE: u32 = 4096;
    const IMAGE_PATH: &str = "/pkg/data/simple_super.img.zstd";

    fn open_image(path: &Path) -> Arc<FakeDevice> {
        let file = std::fs::File::open(path).expect("open file failed");
        let image = zstd::Decoder::new(file).expect("decompress image failed");
        Arc::new(
            FakeDevice::from_image(image, BLOCK_SIZE).expect("create fake block device failed"),
        )
    }

    #[fuchsia::test]
    async fn test_super_parser_get_used_regions() {
        let device = open_image(std::path::Path::new(IMAGE_PATH));
        let super_parser = SuperParser::new(device.clone()).await.expect("SuperParser::new failed");
        let metadata_used_regions = super_parser.metadata_used_regions_in_bytes();
        assert_eq!(metadata_used_regions, vec![(4096..28672)]);

        let partition_used_regions = super_parser
            .partition_used_regions_in_bytes()
            .expect("failed to get partition used regions");
        // This is the expected used region for this test super image. This may need to be updated
        // if the super image changes.
        assert_eq!(partition_used_regions, vec![(1048576..1056768), (2097152..2101248)]);
        device.close().await.expect("failed to close device");
    }

    #[fuchsia::test]
    async fn test_merging_regions() {
        let mut unmerged_regions = BTreeSet::new();
        // Case 1: two adjacent regions
        unmerged_regions.insert(SuperDeviceRange(0..1));
        unmerged_regions.insert(SuperDeviceRange(1..2));
        // Case 2: a fully overlapping region
        unmerged_regions.insert(SuperDeviceRange(0..2));
        // Case 3: a region contained within another
        unmerged_regions.insert(SuperDeviceRange(5..10));
        unmerged_regions.insert(SuperDeviceRange(7..8));
        // Case 4: partially overlapping region
        unmerged_regions.insert(SuperDeviceRange(15..20));
        unmerged_regions.insert(SuperDeviceRange(13..18));
        // Case 5: partially overlapping region (only the ends are different).
        unmerged_regions.insert(SuperDeviceRange(25..27));
        unmerged_regions.insert(SuperDeviceRange(25..30));

        let merged_regions: Vec<SuperDeviceRange> =
            into_merged_regions(unmerged_regions).into_iter().collect();
        assert_eq!(
            merged_regions,
            vec![
                SuperDeviceRange(0..2),
                SuperDeviceRange(5..10),
                SuperDeviceRange(13..20),
                SuperDeviceRange(25..30)
            ]
        );
    }

    #[fuchsia::test]
    async fn test_get_sub_partition() {
        let parent_device = open_image(std::path::Path::new(IMAGE_PATH));

        // The partition contains only zero in the simple test image, add some more interesting bits
        // to make sure we are reading the contents correctly.
        let super_parser =
            SuperParser::new(parent_device.clone()).await.expect("SuperParser::new failed");
        let used_regions = super_parser.partition_used_regions_in_bytes().expect("partitions");
        // We expect the used regions to contain [ region_of_metadata, logical_partitions... ]
        let start_of_first_partition = used_regions[0].start;
        let random_buffer: Vec<u8> = (0..8192).map(|_| rand::random_range(0..100)).collect();
        let mut modified_buffer = parent_device.allocate_buffer(8192).await;
        modified_buffer.copy_from_slice(&random_buffer);
        parent_device
            .write(start_of_first_partition, modified_buffer.as_ref())
            .await
            .expect("failed to write to device");

        let system_partition =
            super_parser.get_sub_partition("system_a", 0).expect("failed to load partition device");
        assert_eq!(system_partition.block_size(), 4096);
        assert_eq!(system_partition.block_count(), 2);
        // Verify the contents of this sub-partition
        let mut read_buffer = system_partition.allocate_buffer(8192).await;
        system_partition.read(0, read_buffer.as_mut()).await.expect("failed to read from device");
        assert_eq!(read_buffer.to_vec(), random_buffer);
        // Check that we can read from a non-0 offset
        let mut read_buffer = system_partition.allocate_buffer(4096).await;
        system_partition
            .read(4096, read_buffer.as_mut())
            .await
            .expect("failed to read from device");
        assert_eq!(read_buffer.to_vec(), random_buffer[4096..]);

        // Read the contents of the next partition. The simple test super image is set up such that
        // this should just be zeroes.
        let system_ext_partition = super_parser
            .get_sub_partition("system_ext_a", 0)
            .expect("failed to load partition device");

        assert_eq!(system_ext_partition.block_size(), 4096);
        assert_eq!(system_ext_partition.block_count(), 1);
        let mut read_buffer = system_ext_partition.allocate_buffer(4096).await;
        system_ext_partition
            .read(0, read_buffer.as_mut())
            .await
            .expect("failed to read from device");
        assert_eq!(read_buffer.to_vec(), [0; 4096]);
    }

    #[fuchsia::test]
    async fn test_misaligned_read_sub_partition_should_fail() {
        let parent_device = open_image(std::path::Path::new(IMAGE_PATH));

        let super_parser =
            SuperParser::new(parent_device.clone()).await.expect("SuperParser::new failed");

        let system_partition =
            super_parser.get_sub_partition("system_a", 0).expect("failed to load partition device");

        // Test reading when buffer is not a multiple of block size
        let mut read_buffer = system_partition.allocate_buffer(3).await;
        system_partition
            .read(0, read_buffer.as_mut())
            .await
            .expect_err("misaligned read from device passes unexpectedly");

        // Test reading from misaligned buffer
        let mut read_buffer = system_partition.allocate_buffer(4096).await;
        system_partition
            .read(7, read_buffer.as_mut())
            .await
            .expect_err("misaligned read from device passes unexpectedly");
    }

    #[fuchsia::test]
    async fn test_out_of_bounds_read_sub_partition_should_fail() {
        let parent_device = open_image(std::path::Path::new(IMAGE_PATH));

        let super_parser =
            SuperParser::new(parent_device.clone()).await.expect("SuperParser::new failed");

        let system_partition =
            super_parser.get_sub_partition("system_a", 0).expect("failed to load partition device");

        let block_size = system_partition.block_size() as usize;
        let block_count = system_partition.block_count() as usize;
        let out_of_bounds_len = (block_count + 1) * (block_size);

        // Test reading when buffer is out of bounds
        let mut read_buffer = system_partition.allocate_buffer(out_of_bounds_len).await;
        system_partition
            .read(0, read_buffer.as_mut())
            .await
            .expect_err("out of bounds read from device passes unexpectedly");

        // Test reading when reading from out of bounds
        let mut read_buffer = system_partition.allocate_buffer(4096).await;
        system_partition
            .read(out_of_bounds_len as u64, read_buffer.as_mut())
            .await
            .expect_err("out of bounds read from device passes unexpectedly");
    }

    const METADATA_MAX_SIZE: u32 = 65536;
    const METADATA_SLOT_COUNT: u32 = 2;

    // To force metadata to span multiple blocks, we need to add enough partitions.
    // Each partition entry is 52 bytes.
    // Metadata header is 256 bytes.
    // Block size is 4096 bytes.
    // To exceed one block: 256 + (N * 52) > 4096 => N > 73.
    const NUM_PARTITIONS_TO_FILL_BLOCK: usize = 74;

    fn create_geometry() -> MetadataGeometry {
        let mut geometry = MetadataGeometry {
            magic: METADATA_GEOMETRY_MAGIC,
            struct_size: std::mem::size_of::<MetadataGeometry>() as u32,
            checksum: [0; 32],
            metadata_max_size: METADATA_MAX_SIZE,
            metadata_slot_count: METADATA_SLOT_COUNT,
            logical_block_size: 4096,
        };
        geometry.checksum = geometry.compute_checksum();
        geometry
    }

    fn create_metadata_header(tables_size: u32) -> MetadataHeader {
        let mut header = MetadataHeader {
            magic: METADATA_HEADER_MAGIC,
            major_version: METADATA_MAJOR_VERSION,
            minor_version: METADATA_VERSION_FOR_EXPANDED_HEADER_MIN,
            header_size: std::mem::size_of::<MetadataHeader>() as u32,
            header_checksum: [0; 32],
            tables_size,
            tables_checksum: [0; 32],
            partitions: MetadataTableDescriptor {
                offset: 0,
                num_entries: 0,
                entry_size: std::mem::size_of::<MetadataPartition>() as u32,
            },
            extents: MetadataTableDescriptor {
                offset: 0,
                num_entries: 0,
                entry_size: std::mem::size_of::<MetadataExtent>() as u32,
            },
            groups: MetadataTableDescriptor {
                offset: 0,
                num_entries: 0,
                entry_size: std::mem::size_of::<MetadataPartitionGroup>() as u32,
            },
            block_devices: MetadataTableDescriptor {
                offset: 0,
                num_entries: 0,
                entry_size: std::mem::size_of::<MetadataBlockDevice>() as u32,
            },
            flags: MetadataHeaderFlags::empty(),
            reserved: [0; 124],
        };
        header.header_checksum = header.compute_checksum();
        header
    }

    struct MetadataBuilder {
        partitions: Vec<MetadataPartition>,
        extents: Vec<MetadataExtent>,
        groups: Vec<MetadataPartitionGroup>,
        block_devices: Vec<MetadataBlockDevice>,
    }

    impl MetadataBuilder {
        fn new() -> Self {
            Self {
                partitions: Vec::new(),
                extents: Vec::new(),
                groups: Vec::new(),
                block_devices: Vec::new(),
            }
        }

        fn add_partition(&mut self, name: &str, attributes: PartitionAttributes, group_index: u32) {
            let mut name_bytes = [0; 36];
            name_bytes[..name.len()].copy_from_slice(name.as_bytes());
            self.partitions.push(MetadataPartition {
                name: name_bytes,
                attributes,
                first_extent_index: self.extents.len() as u32,
                num_extents: 0,
                group_index,
            });
        }

        fn add_extent(&mut self, num_sectors: u64, target_data: u64, target_source: u32) {
            self.extents.push(MetadataExtent {
                num_sectors,
                target_type: TARGET_TYPE_LINEAR,
                target_data,
                target_source,
            });
            if let Some(partition) = self.partitions.last_mut() {
                partition.num_extents += 1;
            }
        }

        fn add_group(&mut self, name: &str, maximum_size: u64) {
            let mut name_bytes = [0; 36];
            name_bytes[..name.len()].copy_from_slice(name.as_bytes());
            self.groups.push(MetadataPartitionGroup {
                name: name_bytes,
                flags: PartitionGroupFlags::empty(),
                maximum_size,
            });
        }

        fn add_block_device(&mut self, name: &str, size: u64, first_logical_sector: u64) {
            let mut name_bytes = [0; 36];
            name_bytes[..name.len()].copy_from_slice(name.as_bytes());
            self.block_devices.push(MetadataBlockDevice {
                first_logical_sector,
                alignment: 1024 * 1024,
                alignment_offset: 0,
                size,
                partition_name: name_bytes,
                flags: BlockDeviceFlags::empty(),
            });
        }

        fn build(self) -> (MetadataHeader, Vec<u8>) {
            let partitions_size = self.partitions.len() * std::mem::size_of::<MetadataPartition>();
            let extents_size = self.extents.len() * std::mem::size_of::<MetadataExtent>();
            let groups_size = self.groups.len() * std::mem::size_of::<MetadataPartitionGroup>();
            let block_devices_size =
                self.block_devices.len() * std::mem::size_of::<MetadataBlockDevice>();

            let tables_size = partitions_size + extents_size + groups_size + block_devices_size;

            let mut tables = Vec::new();
            for p in &self.partitions {
                tables.extend_from_slice(p.as_bytes());
            }
            for e in &self.extents {
                tables.extend_from_slice(e.as_bytes());
            }
            for g in &self.groups {
                tables.extend_from_slice(g.as_bytes());
            }
            for b in &self.block_devices {
                tables.extend_from_slice(b.as_bytes());
            }

            let mut header = create_metadata_header(tables_size as u32);
            header.partitions.num_entries = self.partitions.len() as u32;
            header.extents.offset = partitions_size as u32;
            header.extents.num_entries = self.extents.len() as u32;
            header.groups.offset = (partitions_size + extents_size) as u32;
            header.groups.num_entries = self.groups.len() as u32;
            header.block_devices.offset = (partitions_size + extents_size + groups_size) as u32;
            header.block_devices.num_entries = self.block_devices.len() as u32;

            header.tables_checksum = sha2::Sha256::digest(&tables).into();
            header.header_checksum = header.compute_checksum();

            (header, tables)
        }
    }

    async fn write_metadata_to_device(
        device: &dyn Device,
        geometry: &MetadataGeometry,
        builder: MetadataBuilder,
        slot: u32,
    ) {
        let (header, tables) = builder.build();
        let offset = geometry.get_primary_metadata_offset(slot).unwrap();

        let mut buffer = device.allocate_buffer(geometry.metadata_max_size as usize).await;
        buffer.as_mut_ptr_slice().write(header);
        let header_len = std::mem::size_of::<MetadataHeader>();
        buffer.subslice_mut(header_len..header_len + tables.len()).copy_from_slice(&tables);

        device.write(offset, buffer.as_ref()).await.expect("failed to write metadata");
    }

    #[fuchsia::test]
    async fn test_complex_layout() {
        let device = Arc::new(FakeDevice::new(10 * 1024, BLOCK_SIZE));

        // Write geometry
        let geometry = create_geometry();
        let mut buffer = device.allocate_buffer(BLOCK_SIZE as usize).await;
        buffer.as_mut_ptr_slice().write(geometry);
        device
            .write(PARTITION_RESERVED_BYTES as u64, buffer.as_ref())
            .await
            .expect("failed to write geometry");

        let first_logical_sector = (RESERVED_AND_GEOMETRIES_SIZE
            + (METADATA_MAX_SIZE * METADATA_SLOT_COUNT * 2) as u64)
            / SECTOR_SIZE as u64;

        // Slot 0:
        // - system_a: [sector 2048, length 2048], [sector 8192, length 2048]
        // - vendor_a: [sector 4096, length 2048]
        let mut builder_0 = MetadataBuilder::new();
        builder_0.add_block_device("super", 10 * 1024 * 1024, first_logical_sector);
        builder_0.add_group("default", 0);
        builder_0.add_partition("system_a", PartitionAttributes::empty(), 0);
        builder_0.add_extent(2048, 2048, 0);
        builder_0.add_extent(2048, 8192, 0);
        builder_0.add_partition("vendor_a", PartitionAttributes::empty(), 0);
        builder_0.add_extent(2048, 4096, 0);
        builder_0.add_extent(2048, 4096, 0);

        // Add many partitions to force metadata to span multiple blocks.
        for i in 0..NUM_PARTITIONS_TO_FILL_BLOCK {
            builder_0.add_partition(&format!("dummy_a_{}", i), PartitionAttributes::empty(), 0);
            builder_0.add_extent(1, 10000 + i as u64, 0);
        }

        write_metadata_to_device(device.as_ref(), &geometry, builder_0, 0).await;

        // Slot 1:
        // - system_b: [sector 2048, length 2048], [sector 6144, length 2048]
        // - vendor_b: [sector 4096, length 2048]
        let mut builder_1 = MetadataBuilder::new();
        builder_1.add_block_device("super", 10 * 1024 * 1024, first_logical_sector);
        builder_1.add_group("default", 0);
        builder_1.add_partition("system_b", PartitionAttributes::empty(), 0);
        builder_1.add_extent(2048, 2048, 0);
        builder_1.add_extent(2048, 6144, 0);
        builder_1.add_partition("vendor_b", PartitionAttributes::empty(), 0);
        builder_1.add_extent(2048, 4096, 0);
        builder_1.add_extent(2048, 4096, 0);

        // Add many partitions to force metadata to span multiple blocks.
        for i in 0..NUM_PARTITIONS_TO_FILL_BLOCK {
            builder_1.add_partition(&format!("dummy_b_{}", i), PartitionAttributes::empty(), 0);
            builder_1.add_extent(1, 20000 + i as u64, 0);
        }

        write_metadata_to_device(device.as_ref(), &geometry, builder_1, 1).await;

        let super_parser = SuperParser::new(device.clone()).await.expect("SuperParser::new failed");

        // Verify metadata_used_regions
        let metadata_used_regions = super_parser.metadata_used_regions_in_bytes();
        // Calculate expected metadata size and round up to block size.
        let header_size = std::mem::size_of::<MetadataHeader>();
        let partition_entry_size = std::mem::size_of::<MetadataPartition>();
        let extent_entry_size = std::mem::size_of::<MetadataExtent>();
        let group_entry_size = std::mem::size_of::<MetadataPartitionGroup>();
        let block_device_entry_size = std::mem::size_of::<MetadataBlockDevice>();

        // Base entries: 2 partitions, 3 extents, 1 group, 1 block device
        // Added entries: NUM_PARTITIONS_TO_FILL_BLOCK partitions, NUM_PARTITIONS_TO_FILL_BLOCK extents
        let num_partitions = 2 + NUM_PARTITIONS_TO_FILL_BLOCK;
        let num_extents = 3 + NUM_PARTITIONS_TO_FILL_BLOCK;
        let tables_size = num_partitions * partition_entry_size
            + num_extents * extent_entry_size
            + 1 * group_entry_size
            + 1 * block_device_entry_size;
        let total_metadata_size = header_size + tables_size;

        // Round up to block size
        let metadata_blocks =
            (total_metadata_size as u64 + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64;
        let metadata_size_aligned = metadata_blocks * BLOCK_SIZE as u64;

        assert!(metadata_size_aligned > BLOCK_SIZE as u64);

        let expected_metadata_regions = vec![
            // Geometry
            SuperDeviceRange(PARTITION_RESERVED_BYTES as u64..RESERVED_AND_GEOMETRIES_SIZE),
            // Primary metadata slot 0
            SuperDeviceRange(12288..12288 + metadata_size_aligned),
            // Primary metadata slot 1
            SuperDeviceRange(77824..77824 + metadata_size_aligned),
            // Backup metadata slot 0
            SuperDeviceRange(143360..143360 + metadata_size_aligned),
            // Backup metadata slot 1
            SuperDeviceRange(208896..208896 + metadata_size_aligned),
        ];
        assert_eq!(
            metadata_used_regions,
            into_merged_regions(expected_metadata_regions.into_iter().collect())
                .iter()
                .map(|r| r.0.clone())
                .collect::<Vec<_>>()
        );

        // Verify partition_used_regions (union of extents from both slots)
        let partition_used_regions = super_parser
            .partition_used_regions_in_bytes()
            .expect("failed to get partition used regions");

        // 2048 sectors = 1048576 bytes
        // 4096 sectors = 2097152 bytes
        // 6144 sectors = 3145728 bytes
        // 8192 sectors = 4194304 bytes
        // Length 2048 sectors = 1048576 bytes

        // Slot 0 extents:
        // - [1048576, 2097152)
        // - [4194304, 5242880)
        // - [2097152, 3145728)

        // Slot 1 extents:
        // - [1048576, 2097152)
        // - [3145728, 4194304)
        // - [2097152, 3145728)

        // Union:
        // - [1048576, 3145728)
        // - [3145728, 4194304)
        // - [4194304, 5242880)
        // Merged: [1048576, 5242880)

        // Dummy extents:
        // Slot 0: [5120000, 5120000 + 512 * 74) = [5120000, 5157888)
        // Slot 1: [10240000, 10240000 + 512 * 74) = [10240000, 10277888)

        // Union:
        // - [1048576, 5242880) (from base partitions)
        // - [5120000, 5157888) (dummy slot 0)
        // - [10240000, 10277888) (dummy slot 1)

        // Merged:
        // - [1048576, 5242880) U [5120000, 5157888) = [1048576, 5242880) (placeholder slot 0 is inside)
        // - [10240000, 10277888)

        let expected_partition_regions =
            vec![SuperDeviceRange(1048576..5242880), SuperDeviceRange(10240000..10277888)];
        assert_eq!(
            partition_used_regions,
            expected_partition_regions.iter().map(|r| r.0.clone()).collect::<Vec<_>>()
        );
    }

    #[fuchsia::test]
    async fn test_get_extents() {
        let parent_device = open_image(std::path::Path::new(IMAGE_PATH));
        let super_parser =
            SuperParser::new(parent_device.clone()).await.expect("SuperParser::new failed");

        let system_partition =
            super_parser.get_sub_partition("system_a", 0).expect("failed to load partition device");

        // Let's test `system_a` for a subset.
        let extents = system_partition.get_extents_for_range(0..4096).expect("valid range");
        assert_eq!(extents, vec![1048576..1052672]);

        let extents = system_partition.get_extents_for_range(0..8192).expect("valid range");
        assert_eq!(extents, vec![1048576..1056768]);

        // Partial
        let extents = system_partition.get_extents_for_range(100..4200).expect("valid range");
        // Should range from 1048576 + 100 .. 1048576 + 4200
        assert_eq!(extents, vec![1048676..1052776]);

        // Out of bounds
        system_partition
            .get_extents_for_range(8000..9000)
            .expect_err("should return error for sparse/oob");

        // Completely out of bounds
        system_partition
            .get_extents_for_range(9000..10000)
            .expect_err("should return error for oob");

        // Test a fragmented partition.
        // We can't build a "super" partition but we can fake up a SuperPartitionDevice for this.
        let device = Arc::new(FakeDevice::new(5000, 4096));
        let extent_locations = vec![
            SuperDeviceRange(1000..2000), // 1000 bytes
            SuperDeviceRange(5000..5500), // 500 bytes
            SuperDeviceRange(4000..4500), // 500 bytes
        ];
        let partition =
            SuperPartitionDevice::new(device.clone(), extent_locations).expect("new failed");

        // Test spanning
        let extents = partition.get_extents_for_range(500..1500).expect("valid spanning range");
        // Should be [1500..2000, 5000..5500]
        assert_eq!(extents, vec![1500..2000, 5000..5500]);

        // Test spanning exact boundaries
        let extents = partition.get_extents_for_range(1000..2000).expect("valid range");
        assert_eq!(extents, vec![5000..5500, 4000..4500]);

        let extents = partition.get_extents_for_range(999..1001).expect("valid range");
        assert_eq!(extents, vec![1999..2000, 5000..5001]);

        // Test crossing from second to third extent (out of order)
        let extents = partition.get_extents_for_range(1400..1600).expect("valid range");
        assert_eq!(extents, vec![5400..5500, 4000..4100]);
    }
}
