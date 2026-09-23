// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::format::{
    AdjustSlotSuffix, METADATA_GEOMETRY_RESERVED_SIZE, MetadataBlockDevice, MetadataExtent,
    MetadataGeometry, MetadataHeader, MetadataPartition, MetadataPartitionGroup,
    MetadataTableDescriptor, PARTITION_RESERVED_BYTES, ValidateTable,
};
use anyhow::{Error, anyhow, ensure};
use sha2::Digest;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::ops::{Deref, DerefMut, Range};
use storage_device::Device;
use zerocopy::{FromBytes, KnownLayout};

// Same as `std::ops::Range` but has support for `std::cmp::Ord` and `std::cmp::PartialOrd` (sort
// first by start of the range then by the end). Due to Rust's orphan rule, we cannot just implement
// these traits for std::ops::Range`. Define a struct containing Range so that we own the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuperDeviceRange(pub Range<u64>);

impl Deref for SuperDeviceRange {
    type Target = Range<u64>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SuperDeviceRange {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Range<u64>> for SuperDeviceRange {
    fn from(range: Range<u64>) -> Self {
        Self(range)
    }
}

impl Into<Range<u64>> for SuperDeviceRange {
    fn into(self) -> Range<u64> {
        self.0
    }
}

impl Ord for SuperDeviceRange {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start.cmp(&other.start).then(self.end.cmp(&other.end))
    }
}

impl PartialOrd for SuperDeviceRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(crate) fn round_up_to_alignment(x: u64, alignment: u64) -> Result<u64, Error> {
    ensure!(alignment.count_ones() == 1, "alignment should be a power of 2");
    alignment
        .checked_mul(x.div_ceil(alignment))
        .ok_or_else(|| anyhow!("overflow occurred when rounding up to nearest alignment"))
}

// TODO(https://fxbug.dev/404952286): Add fuzzer to check for arithmetic overflows.

/// Struct to help interpret the deserialized "super" metadata region. "metadata" is an overloaded
/// term. The metadata region of "super" looks like the following:
///     +-----------------------------------+
///     | Reserved bytes                    |
///     +-----------------------------------+
///     | Geometry                          |
///     +-----------------------------------+
///     | Geometry Backup                   |
///     +-----------------------------------+
///     | Metadata                          |
///     |                                   |
///     |  * contains `metadata_slot_count` |
///     |    copies of the metadata         |
///     |                                   |
///     +-----------------------------------+
///     | Backup Metadata                   |
///     | ...                               |
///     +-----------------------------------+
/// The `Metadata` entry contains metadata for the partitions, extents, partition groups and block
/// devices. There may be multiple copies (known as "slots") of it. For example, an A/B device would
/// have 2 slots. To differentiate between the two "metadata" - the super metadata region will be
/// referred to as "SuperMetadata" and the latter metadata containing information on the partitios,
/// extents, etc. will be referred to as "Metadata".
#[derive(Debug)]
pub struct SuperMetadata {
    pub geometry: MetadataGeometry,
    pub metadata_slots: Vec<Metadata>,
}

impl SuperMetadata {
    pub async fn load_from_device(device: &dyn Device) -> Result<Self, Error> {
        let geometry = Self::load_metadata_geometry_from_device(device).await?;
        let slot_count = geometry.metadata_slot_count;

        let mut metadata_slots = Vec::new();
        for slot in 0..slot_count {
            metadata_slots.push(Metadata::load_from_device(device, &geometry, slot).await?);
        }

        Ok(Self { geometry, metadata_slots: metadata_slots })
    }

    pub fn block_size(&self) -> u32 {
        self.geometry.logical_block_size
    }

    // Load and validate geometry information from a block device that holds logical partitions.
    async fn load_metadata_geometry_from_device(
        device: &dyn Device,
    ) -> Result<MetadataGeometry, Error> {
        // Read the primary geometry
        match Self::parse_metadata_geometry(device, PARTITION_RESERVED_BYTES as u64).await {
            Ok(geometry) => Ok(geometry),
            Err(_) => {
                // Try the backup geometry if the primary geometry fails.
                Self::parse_metadata_geometry(
                    device,
                    PARTITION_RESERVED_BYTES as u64 + METADATA_GEOMETRY_RESERVED_SIZE as u64,
                )
                .await
            }
        }
    }

    // Read and validates the metadata geometry. The offset provided will be rounded up to the
    // nearest block alignment.
    async fn parse_metadata_geometry(
        device: &dyn Device,
        offset: u64,
    ) -> Result<MetadataGeometry, Error> {
        let buffer_len = round_up_to_alignment(
            METADATA_GEOMETRY_RESERVED_SIZE as u64,
            device.block_size() as u64,
        )?;
        let mut buffer = device.allocate_buffer(buffer_len.try_into()?).await;
        let aligned_offset = round_up_to_alignment(offset, device.block_size() as u64)?;
        device.read(aligned_offset, buffer.as_mut()).await?;
        let metadata_geometry: MetadataGeometry = buffer
            .as_ptr_slice()
            .read()
            .ok_or_else(|| anyhow!("Failed to read metadata geometry"))?;
        metadata_geometry.validate()?;
        Ok(metadata_geometry)
    }
}

/// Struct to help interpret the deserialized metadata.
#[derive(Debug)]
pub struct Metadata {
    header: MetadataHeader,
    partitions: HashMap<String, MetadataPartition>,
    extents: Vec<MetadataExtent>,
    partition_groups: Vec<MetadataPartitionGroup>,
    block_devices: Vec<MetadataBlockDevice>,
}

impl Metadata {
    pub fn header(&self) -> &MetadataHeader {
        &self.header
    }

    pub fn partitions(&self) -> &HashMap<String, MetadataPartition> {
        &self.partitions
    }

    /// Caller should indicate which slot_number to read from. For example, for an A/B device, there
    /// will be two variants of metadata: "slot_a" is stored at slot 0 and "slot_b" is stored at
    /// slot 1. For devices where there slot count is 0, metadata will be stored at slot 0.
    pub async fn load_from_device(
        device: &dyn Device,
        geometry: &MetadataGeometry,
        slot_number: u32,
    ) -> Result<Self, Error> {
        let mut metadata = match Self::load_metadata_from_device(
            device,
            geometry,
            geometry.get_primary_metadata_offset(slot_number)?,
        )
        .await
        {
            Ok(metadata) => metadata,
            Err(_) => {
                Self::load_metadata_from_device(
                    device,
                    geometry,
                    geometry.get_backup_metadata_offset(slot_number)?,
                )
                .await?
            }
        };

        // Need to update names for slot suffix before returning metadata.
        metadata.adjust_for_slot_suffix(slot_number)?;

        // TODO(https://fxbug.dev/404952286): Check for unique partition name

        Ok(metadata)
    }

    // Returns the locations of extents used by partitions.
    pub fn get_all_used_extents_as_byte_range(&self) -> Result<BTreeSet<SuperDeviceRange>, Error> {
        let mut used_regions = BTreeSet::new();
        for (partition, _partition_metadata) in &self.partitions {
            let used_extents = self.extent_locations_for_partition(&partition)?;
            for extent in used_extents {
                used_regions.insert(extent);
            }
        }
        Ok(used_regions)
    }

    // Returns the list of extent locations as a byte range for a given partition.
    pub fn extent_locations_for_partition(
        &self,
        partition_name: &str,
    ) -> Result<Vec<SuperDeviceRange>, Error> {
        let partition_metadata = self
            .partitions
            .get(partition_name)
            .ok_or_else(|| anyhow!("Could not find partition with name {partition_name}"))?;

        let mut extent_locations = Vec::new();
        let first_extent_index = partition_metadata.first_extent_index;
        for extent_offset in 0..partition_metadata.num_extents {
            let extent_index = (first_extent_index as usize)
                .checked_add(extent_offset as usize)
                .ok_or_else(|| anyhow!("failed to get extent index"))?;
            extent_locations.push(self.extents[extent_index].as_byte_range()?);
        }
        Ok(extent_locations)
    }

    async fn load_metadata_from_device(
        device: &dyn Device,
        geometry: &MetadataGeometry,
        offset: u64,
    ) -> Result<Self, Error> {
        let header = Self::parse_metadata_header(device, offset).await?;

        // Now that we have more information on the tables, validate table-specific fields.
        ensure!(header.tables_size <= geometry.metadata_max_size, "Invalid tables size.");

        // Read tables bytes from device
        ensure!(offset % device.block_size() as u64 == 0, "Reads must be block aligned.");
        let header_and_tables_size = header
            .header_size
            .checked_add(header.tables_size)
            .ok_or_else(|| anyhow!("calculate header and tables size: arithmetic overflow"))?;
        let buffer_len =
            round_up_to_alignment(header_and_tables_size as u64, device.block_size() as u64)?;
        let mut buffer = device.allocate_buffer(buffer_len.try_into()?).await;
        device.read(offset, buffer.as_mut()).await?;
        let data = buffer.to_vec();
        let tables_bytes =
            &data[header.header_size.try_into()?..header_and_tables_size.try_into()?];

        // Read tables to verify `table_checksum` in metadata_header.
        let computed_tables_checksum: [u8; 32] = sha2::Sha256::digest(tables_bytes).into();
        ensure!(
            computed_tables_checksum == header.tables_checksum,
            "Invalid metadata tables checksum"
        );

        // Parse partition table entries.
        let read_cursor = 0;
        let partitions: HashMap<String, MetadataPartition> =
            Self::parse_table::<MetadataPartition>(
                tables_bytes,
                read_cursor,
                &header,
                &header.partitions,
            )?
            .iter()
            // Calling `unwrap` on `trimmed_name` should be fine as the partition metadata has
            // already been validated and checked that calling `trimmed_name` will not return an
            // error.
            .map(|p| (p.trimmed_name().unwrap(), *p))
            .collect();

        // Parse extent table entries.
        let read_cursor =
            read_cursor.checked_add(header.partitions.get_table_size()?).ok_or_else(|| {
                anyhow!("calculate cursor to read extent entries: arithmetic overflow")
            })?;
        let extents = Self::parse_table::<MetadataExtent>(
            tables_bytes,
            read_cursor,
            &header,
            &header.extents,
        )?;

        // Parse partition group table entries.
        let read_cursor =
            read_cursor.checked_add(header.extents.get_table_size()?).ok_or_else(|| {
                anyhow!("calculate cursor to read group entries: arithmetic overflow")
            })?;
        let partition_groups = Self::parse_table::<MetadataPartitionGroup>(
            tables_bytes,
            read_cursor,
            &header,
            &header.groups,
        )?;

        // Parse block device table entries.
        let read_cursor =
            read_cursor.checked_add(header.groups.get_table_size()?).ok_or_else(|| {
                anyhow!("calculate cursor to read block devices entries: arithmetic overflow")
            })?;
        let block_devices = Self::parse_table::<MetadataBlockDevice>(
            tables_bytes,
            read_cursor,
            &header,
            &header.block_devices,
        )?;

        // Expect there to be at least be one block device: "super".
        ensure!(block_devices.len() > 0, "Metadata did not specify a super device.");
        let super_device = block_devices[0];
        let logical_partition_offset = super_device.get_first_logical_sector_in_bytes()?;
        let metadata_region = geometry.get_total_metadata_size()?;
        ensure!(
            metadata_region <= logical_partition_offset,
            "Logical partition metadata overlaps with logical partition contents."
        );

        Ok(Self { header, partitions, extents, partition_groups, block_devices })
    }

    async fn parse_metadata_header(
        device: &dyn Device,
        offset: u64,
    ) -> Result<MetadataHeader, Error> {
        // Reads must be block aligned.
        ensure!(offset % device.block_size() as u64 == 0, "Reads must be block aligned.");
        let buffer_len = round_up_to_alignment(
            std::mem::size_of::<MetadataHeader>() as u64,
            device.block_size() as u64,
        )?;
        let mut buffer = device.allocate_buffer(buffer_len.try_into()?).await;
        device.read(offset, buffer.as_mut()).await?;

        let mut metadata_header: MetadataHeader = buffer
            .as_ptr_slice()
            .read()
            .ok_or_else(|| anyhow!("Failed to read metadata header"))?;
        // Validation will also check if the header is an older version, and if so, will zero the
        // fields in `MetadataHeader` that did not exist in the older version.
        metadata_header
            .validate()
            .map_err(|e| anyhow!("failed metadata header validation: {e}"))?;
        Ok(metadata_header)
    }

    fn parse_table<T: ValidateTable + FromBytes + KnownLayout>(
        tables_bytes: &[u8],
        tables_offset: u32,
        metadata_header: &MetadataHeader,
        table_descriptor: &MetadataTableDescriptor,
    ) -> Result<Vec<T>, Error> {
        let mut entries = Vec::new();
        let entry_size = table_descriptor.entry_size;
        let num_entries = table_descriptor.num_entries;
        for index in 0..num_entries {
            let read_so_far = index
                .checked_mul(entry_size)
                .ok_or_else(|| anyhow!("calculate bytes read so far: arithmetic overflow"))?;
            let offset = tables_offset
                .checked_add(read_so_far)
                .ok_or_else(|| anyhow!("calculate table bytes offset: arithmetic overflow"))?;
            let (entry, _remainder) = T::read_from_prefix(&tables_bytes[offset.try_into()?..])
                .map_err(|e| anyhow!("Failed to read table: {e}"))?;
            entry.validate(metadata_header)?;
            entries.push(entry);
        }
        Ok(entries)
    }

    fn adjust_for_slot_suffix(&mut self, slot_number: u32) -> Result<(), Error> {
        let old_partition_names: Vec<String> =
            self.partitions.iter().map(|(k, _v)| k.clone()).collect();
        for old_partition_name in old_partition_names {
            // Unwrapping here should be fine as we know that the map contains this key.
            let mut partition_metadata = self.partitions.remove(&old_partition_name).unwrap();
            partition_metadata.adjust_for_slot_suffix(slot_number)?;
            partition_metadata.validate(&self.header)?;
            self.partitions
                .insert(partition_metadata.trimmed_name().unwrap(), partition_metadata.clone());
        }

        for block_device in &mut self.block_devices {
            block_device.adjust_for_slot_suffix(slot_number)?;
        }

        for partition_group in &mut self.partition_groups {
            partition_group.adjust_for_slot_suffix(slot_number)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::format::{
        BlockDeviceFlags, MetadataHeaderFlags, PartitionAttributes, PartitionGroupFlags,
        RESERVED_AND_GEOMETRIES_SIZE, SECTOR_SIZE,
    };

    use super::*;
    use std::path::Path;
    use std::sync::Arc;
    use storage_device::fake_device::FakeDevice;

    const BLOCK_SIZE: u32 = 4096;
    const IMAGE_PATH: &str = "/pkg/data/simple_super.img.zstd";
    const IMAGE_METADATA_MAX_SIZE: u32 = 4096;

    fn open_image(path: &Path) -> Arc<FakeDevice> {
        // If image changes at the file path, need to update the `verify_*` functions below that
        // verifies the parser and the test const `IMAGE_METADATA_MAX_SIZE`.
        let file = std::fs::File::open(path).expect("open file failed");
        let image = zstd::Decoder::new(file).expect("decompress image failed");
        Arc::new(
            FakeDevice::from_image(image, BLOCK_SIZE).expect("create fake block device failed"),
        )
    }

    // Verify metadata geometry against the image at `IMAGE_PATH`.
    fn verify_geometry(geometry: &MetadataGeometry) -> Result<(), Error> {
        let max_size = geometry.metadata_max_size;
        let metadata_slot_count = geometry.metadata_slot_count;
        assert_eq!(max_size, IMAGE_METADATA_MAX_SIZE);
        assert_eq!(metadata_slot_count, 2);
        Ok(())
    }

    // Verify metadata header against the image at `IMAGE_PATH`.
    fn verify_header(metadata: &Metadata) -> Result<(), Error> {
        let header = metadata.header;
        let num_partition_entries = header.partitions.num_entries;
        let num_extent_entries = header.extents.num_entries;
        let flags = header.flags;
        assert_eq!(num_partition_entries, 2);
        assert_eq!(num_extent_entries, 2);
        assert!(flags.contains(MetadataHeaderFlags::VIRTUAL_AB_DEVICE));
        Ok(())
    }

    // Verify metadata partitions table against the image at `IMAGE_PATH`.
    fn verify_partitions_table(metadata: &Metadata, slot_number: usize) -> Result<(), Error> {
        let mut partitions = metadata.partitions.clone();
        let expected_partitions = match slot_number {
            0 => ["system_a".to_string(), "system_ext_a".to_string()],
            1 => ["system_b".to_string(), "system_ext_b".to_string()],
            _ => return Err(anyhow!("unexpected slot number: should be 0 or 1")),
        };
        for expected_partition in expected_partitions {
            let (name, partition_metadata) = partitions
                .remove_entry(&expected_partition)
                .ok_or_else(|| anyhow!("could not find {expected_partition}"))?;
            let attributes = partition_metadata.attributes;
            assert_eq!(attributes, PartitionAttributes::READONLY);
            // The slot suffix flag should be removed after parsing Metadata.
            let attributes = partition_metadata.attributes;
            assert!(!attributes.contains(PartitionAttributes::SLOT_SUFFIXED));
            // Check partition extent information
            let system_partition_extent_index = partition_metadata.first_extent_index;
            let extent_locations = metadata.extent_locations_for_partition(&name)?;
            if !name.contains("ext") {
                assert_eq!(system_partition_extent_index, 0);
                assert_eq!(extent_locations, [SuperDeviceRange(1048576..1056768)]);
            } else {
                assert_eq!(system_partition_extent_index, 1);
                assert_eq!(extent_locations, [SuperDeviceRange(2097152..2101248)]);
            }
            let system_partition_num_extents = partition_metadata.num_extents;
            assert_eq!(system_partition_num_extents, 1);
        }
        Ok(())
    }

    // Verify metadata extents table against the image at `IMAGE_PATH`.
    fn verify_extents_table(metadata: &Metadata) -> Result<(), Error> {
        // The simple super image has a "system" partition of size 8192 bytes. This extent entry
        // refers to the extent used by that partition.
        let num_sectors = metadata.extents[0].num_sectors;
        assert_eq!(num_sectors * SECTOR_SIZE as u64, 8192);

        // The simple super image has a "system_ext" partition of size 4096 bytes. This extent entry
        // refers to the extent used by that partition.
        let num_sectors = metadata.extents[1].num_sectors;
        assert_eq!(num_sectors * SECTOR_SIZE as u64, 4096);
        Ok(())
    }

    // Verify metadata partition groups table against the image at `IMAGE_PATH`.
    fn verify_partition_groups_table(metadata: &Metadata, slot_numer: usize) -> Result<(), Error> {
        // Expect to see two groups, "default" of unlimited maximum size and "example" of size 0.
        assert_eq!(metadata.partition_groups.len(), 2);

        // The default group does not have a slot suffix applied.
        let default_group = metadata.partition_groups[0];
        let name = String::from_utf8(default_group.name.to_vec())
            .expect("failed to convert partition group name to string");
        assert_eq!(name.trim_end_matches(|c| c == '\0'), "default".to_string());
        let maximum_size = default_group.maximum_size;
        assert_eq!(maximum_size, 0);

        let group = metadata.partition_groups[1];
        let name = String::from_utf8(group.name.to_vec())
            .expect("failed to convert partition group name to string");
        let expected_name = match slot_numer {
            0 => "example_a".to_string(),
            1 => "example_b".to_string(),
            _ => return Err(anyhow!("unexpected slot number: should be 0 or 1")),
        };
        assert_eq!(name.trim_end_matches(|c| c == '\0'), expected_name);
        let maximum_size = group.maximum_size;
        assert_eq!(maximum_size, 0);

        // The slot suffix flag should be removed after parsing Metadata.
        let group_flag = group.flags;
        assert!(!group_flag.contains(PartitionGroupFlags::SLOT_SUFFIXED));
        Ok(())
    }

    // Verify metadata block devices table against the image at `IMAGE_PATH`.
    fn verify_block_devices_table(metadata: &Metadata, slot_number: usize) -> Result<(), Error> {
        let block_devices = &metadata.block_devices;
        let partition_name = String::from_utf8(block_devices[0].partition_name.to_vec())
            .expect("failed to convert partition entry name to string");
        let expected_name = match slot_number {
            0 => "super_a".to_string(),
            1 => "super_b".to_string(),
            _ => return Err(anyhow!("unexpected slot number: should be 0 or 1")),
        };
        assert_eq!(partition_name.trim_end_matches(|c| c == '\0'), expected_name);
        let device_size = block_devices[0].size;
        assert_eq!(device_size, 4194304);
        let flag = block_devices[0].flags;
        assert!(!flag.contains(BlockDeviceFlags::SLOT_SUFFIXED));
        assert_eq!(block_devices[0].get_first_logical_sector_in_bytes()?, 1048576);
        Ok(())
    }

    fn verify_metadata(super_metadata: &SuperMetadata, slot_number: usize) -> Result<(), Error> {
        verify_geometry(&super_metadata.geometry).expect("incorrect geometry");
        let metadata_slot = &super_metadata.metadata_slots[slot_number];
        verify_header(metadata_slot).expect("incorrect header");
        verify_partitions_table(metadata_slot, slot_number).expect("incorrect partitions table");
        verify_extents_table(metadata_slot).expect("incorrect extents table");
        verify_partition_groups_table(metadata_slot, slot_number)
            .expect("incorrect partition groups table");
        verify_block_devices_table(metadata_slot, slot_number)
            .expect("incorrect block devices table");
        Ok(())
    }

    #[fuchsia::test]
    async fn test_loading_super_metadata() {
        let device = open_image(std::path::Path::new(IMAGE_PATH));

        let super_metadata = SuperMetadata::load_from_device(device.as_ref())
            .await
            .expect("failed to load super metadata");

        verify_metadata(&super_metadata, 0).expect("incorrect metadata");
        device.close().await.expect("failed to close device");
    }

    #[fuchsia::test]
    async fn test_loading_super_metadata_with_invalid_primary_geometry() {
        let device = open_image(std::path::Path::new(IMAGE_PATH));

        // Corrupt the primary geometry bytes.
        {
            let offset =
                round_up_to_alignment(PARTITION_RESERVED_BYTES as u64, device.block_size() as u64)
                    .expect("failed to round to nearest block");
            let buf_len = round_up_to_alignment(
                METADATA_GEOMETRY_RESERVED_SIZE as u64,
                device.block_size() as u64,
            )
            .expect("failed to round to nearest block");
            let mut buf = device.allocate_buffer(buf_len as usize).await;
            buf.fill(0xaa);
            device.write(offset, buf.as_ref()).await.expect("failed to write to device");
        }

        // Super metadata should still be able to loaded using the backup copy.
        let super_metadata = SuperMetadata::load_from_device(device.as_ref())
            .await
            .expect("failed to load super metadata");
        verify_metadata(&super_metadata, 0).expect("incorrect metadata");
        device.close().await.expect("failed to close device");
    }

    #[fuchsia::test]
    async fn test_loading_super_metadata_with_invalid_primary_and_backup_geometry() {
        let device = open_image(std::path::Path::new(IMAGE_PATH));

        // Corrupt the primary and backup geometry bytes.
        {
            let offset =
                round_up_to_alignment(PARTITION_RESERVED_BYTES as u64, device.block_size() as u64)
                    .expect("failed to round to nearest block");
            let buf_len = round_up_to_alignment(
                2 * METADATA_GEOMETRY_RESERVED_SIZE as u64,
                device.block_size() as u64,
            )
            .expect("failed to round to nearest block");
            let mut buf = device.allocate_buffer(buf_len as usize).await;
            buf.fill(0xaa);
            device.write(offset, buf.as_ref()).await.expect("failed to write to device");
        }

        SuperMetadata::load_from_device(device.as_ref())
            .await
            .expect_err("passed loading super metadata unexpectedly");
        device.close().await.expect("failed to close device");
    }

    #[fuchsia::test]
    async fn test_loading_metadata_slot_with_invalid_slot_number() {
        let device = open_image(std::path::Path::new(IMAGE_PATH));

        let super_metadata = SuperMetadata::load_from_device(device.as_ref())
            .await
            .expect("failed to load super metadata");
        let geometry = super_metadata.geometry;

        // This image has 2 metadata slots. Should not be able to read from a fifth slot.
        Metadata::load_from_device(device.as_ref(), &geometry, 5)
            .await
            .expect_err("passed loading metadata unexpectedly");
        device.close().await.expect("failed to close device");
    }

    #[fuchsia::test]
    async fn test_loading_super_metadata_with_invalid_primary_metadata() {
        let device = open_image(std::path::Path::new(IMAGE_PATH));

        // Corrupt only the primary metadata bytes of both slots.
        {
            let offset = round_up_to_alignment(
                RESERVED_AND_GEOMETRIES_SIZE as u64,
                device.block_size() as u64,
            )
            .expect("failed to round to nearest block");
            // The size of each metadata slots is `IMAGE_METADATA_MAX_SIZE`.
            let buf_len = round_up_to_alignment(
                2 * IMAGE_METADATA_MAX_SIZE as u64,
                device.block_size() as u64,
            )
            .expect("failed to round to nearest block");
            let mut buf = device.allocate_buffer(buf_len as usize).await;
            buf.fill(0xaa);
            device.write(offset, buf.as_ref()).await.expect("failed to write to device");
        }

        let super_metadata = SuperMetadata::load_from_device(device.as_ref())
            .await
            .expect("failed to load super metadata");
        verify_metadata(&super_metadata, 0).expect("incorrect metadata");
        verify_metadata(&super_metadata, 1).expect("incorrect metadata");
    }

    #[fuchsia::test]
    async fn test_loading_super_metadata_with_invalid_primary_and_backup_metadata() {
        let device = open_image(std::path::Path::new(IMAGE_PATH));

        // Corrupt both the primary and backup metadata bytes.
        {
            let offset = round_up_to_alignment(
                RESERVED_AND_GEOMETRIES_SIZE as u64,
                device.block_size() as u64,
            )
            .expect("failed to round to nearest block");
            let buf_len = round_up_to_alignment(
                4 * IMAGE_METADATA_MAX_SIZE as u64,
                device.block_size() as u64,
            )
            .expect("failed to round to nearest block");
            let mut buf = device.allocate_buffer(buf_len as usize).await;
            buf.fill(0xaa);
            device.write(offset, buf.as_ref()).await.expect("failed to write to device");
        }

        SuperMetadata::load_from_device(device.as_ref())
            .await
            .expect_err("passed loading super metadata unexpectedly");
        device.close().await.expect("failed to close device");
    }
}
