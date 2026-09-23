// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::SuperDeviceRange;
use anyhow::{Error, anyhow, ensure};
use bitflags::bitflags;
use sha2::Digest;
use static_assertions::const_assert;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub const PARTITION_RESERVED_BYTES: u32 = 4096;
pub const METADATA_GEOMETRY_MAGIC: u32 = 0x616c4467;
pub const METADATA_HEADER_MAGIC: u32 = 0x414C5030;
pub const SECTOR_SIZE: u32 = 512;
pub const METADATA_GEOMETRY_RESERVED_SIZE: u32 = 4096;

pub const METADATA_MAJOR_VERSION: u16 = 10;
pub const METADATA_MINOR_VERSION_MAX: u16 = 2;

/// The minimum metadata version required for the current, expanded, metadata header struct.
pub const METADATA_VERSION_FOR_EXPANDED_HEADER_MIN: u16 = 2;
/// The minimum metadata version required for the current attributes defined in
/// `PARTITION_ATTRIBUTE_MASK`. Below this version, the accepted attributes are defined in
/// `PARTITION_ATTRIBUTE_MASK_V0`.
pub const METADATA_VERSION_FOR_UPDATED_ATTRIBUTES_MIN: u16 = 1;

// `PARTITION_RESERVED_BYTES + 2 * METADATA_GEOMETRY_RESERVED_SIZE` is used frequently to calculate
// offsets.
pub const RESERVED_AND_GEOMETRIES_SIZE: u64 =
    PARTITION_RESERVED_BYTES as u64 + 2 * METADATA_GEOMETRY_RESERVED_SIZE as u64;

/// `MetadataGeometry` provides information on the location of logical partitions. This struct is
/// stored at block 0 of the first 4096 bytes of the partition.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MetadataGeometry {
    /// Magic number. Should be `METADATA_GEOMETRY_MAGIC`.
    pub magic: u32,
    /// Size of the metadata geometry size in bytes.
    pub struct_size: u32,
    /// SHA256 checksum of this struct, with this field set to 0.
    pub checksum: [u8; 32],
    /// Maximum size, in bytes, of the metadata. Must be a multiple of `SECTOR_SIZE`.
    pub metadata_max_size: u32,
    /// Number of metadata copies to keep. A backup copy of each slot is kept. So, if the slot
    /// count is "2", there will be a total of four copies.
    pub metadata_slot_count: u32,
    /// The logical block size in bytes. Must be a multiple of `SECTOR_SIZE`.
    pub logical_block_size: u32,
}
const_assert!(std::mem::size_of::<MetadataGeometry>() as u32 <= METADATA_GEOMETRY_RESERVED_SIZE);

impl MetadataGeometry {
    pub fn compute_checksum(&self) -> [u8; 32] {
        // The `checksum` field is expected to be zero when computing the SHA256 checksum.
        let mut temp_metadata_geometry = self.clone();
        temp_metadata_geometry.checksum = [0; 32];
        sha2::Sha256::digest(temp_metadata_geometry.as_bytes()).into()
    }

    // Returns the total super metadata size (including the reserved bytes, geometry, and backup
    // metadata copies). Returns an error if there is an overflow.
    pub fn get_total_metadata_size(&self) -> Result<u64, Error> {
        // Metadata region looks like:
        //     +-----------------------------------+
        //     | Reserved bytes                    |
        //     +-----------------------------------+
        //     | Geometry                          |
        //     +-----------------------------------+
        //     | Geometry Backup                   |
        //     +-----------------------------------+
        //     | Metadata                          |
        //     |                                   |
        //     |  * contains `metadata_slot_count` |
        //     |    copies of the metadata         |
        //     |                                   |
        //     +-----------------------------------+
        //     | Backup Metadata                   |
        //     | ...                               |
        //     +-----------------------------------+
        //
        let metadata_size = (self.metadata_max_size as u64)
            .checked_mul(self.metadata_slot_count as u64)
            .ok_or_else(|| anyhow!("calculate metadata size: arithmetic overflow"))?;
        let primary_and_backup_metadata_size = metadata_size.checked_mul(2).ok_or_else(|| {
            anyhow!("calculate primary and backup metadata size: arithmetic overflow")
        })?;
        RESERVED_AND_GEOMETRIES_SIZE
            .checked_add(primary_and_backup_metadata_size as u64)
            .ok_or_else(|| anyhow!("calculate total metadata size: arithmetic overflow"))
    }

    // As it currently is with the values set for const `PARTITION_RESERVED_BYTES` and
    // `METADATA_GEOMETRY_RESERVED_SIZE`, calculating the primary metadata offset would not cause
    // an arithmetic overflow. However, use checked arithmetic operation anyway in case it changes
    // in the future.
    pub fn get_primary_metadata_offset(&self, slot_number: u32) -> Result<u64, Error> {
        let slot_offset =
            (self.metadata_max_size as u64).checked_mul(slot_number as u64).ok_or_else(|| {
                anyhow!("calculate primary metadata slot offset: arithmetic overflow")
            })?;
        RESERVED_AND_GEOMETRIES_SIZE
            .checked_add(slot_offset)
            .ok_or_else(|| anyhow!("calculate primary metadata offset: arithmetic overflow"))
    }

    // As it currently is with the values set for const `PARTITION_RESERVED_BYTES` and
    // `METADATA_GEOMETRY_RESERVED_SIZE`, calculating the backup metadata offset would not cause
    // an arithmetic overflow. However, use checked arithmetic operation anyway in case it changes
    // in the future.
    pub fn get_backup_metadata_offset(&self, slot_number: u32) -> Result<u64, Error> {
        let metadata_size = (self.metadata_max_size as u64)
            .checked_mul(self.metadata_slot_count as u64)
            .ok_or_else(|| anyhow!("calculate backup metadata size: arithmetic overflow"))?;
        let backup_metadata_offset =
            RESERVED_AND_GEOMETRIES_SIZE.checked_add(metadata_size).ok_or_else(|| {
                anyhow!("calculate backup metadata start offset: arithmetic overflow")
            })?;
        let slot_offset = (self.metadata_max_size as u64)
            .checked_mul(slot_number as u64)
            .ok_or_else(|| anyhow!("calculate backup metadata slot offset: arithmetic overflow"))?;
        backup_metadata_offset
            .checked_add(slot_offset)
            .ok_or_else(|| anyhow!("calculate backup metadata offset: arithmetic overflow"))
    }

    pub fn validate(&self) -> Result<(), Error> {
        ensure!(self.magic == METADATA_GEOMETRY_MAGIC, "Invalid metadata geometry magic.");
        ensure!(
            self.struct_size == std::mem::size_of::<MetadataGeometry>() as u32,
            "Invalid metadata geometry struct size."
        );
        ensure!(self.checksum == self.compute_checksum(), "Invalid metadata geometry checksum.");
        ensure!(self.metadata_slot_count > 0, "Invalid metadata slot count. Must be more than 0.");
        ensure!(
            self.metadata_max_size % SECTOR_SIZE == 0,
            "Invalid metadata maximum size. Must be sector-aligned."
        );
        ensure!(
            self.logical_block_size % SECTOR_SIZE == 0,
            "Invalid logical block size. Must be sector-aligned."
        );

        // Check for potential arithmetic overflow.
        ensure!(self.get_total_metadata_size().is_ok(), "Invalid metadata region size.");
        Ok(())
    }
}

/// Header of the metadata format. See `MetadataHeaderV1` for the older compatible version.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MetadataHeader {
    /// Magic number. Should be `METADATA_HEADER_MAGIC`.
    pub magic: u32,
    /// This struct is versioned. If major version is not METADATA_MAJOR_VERSION, the struct is not
    /// compatible.
    pub major_version: u16,
    /// This struct is versioned. Older minor versions are supported.
    pub minor_version: u16,
    /// Size of the metadata header struct in bytes.
    pub header_size: u32,
    /// SHA256 checksum of this struct, with this field set to zero.
    pub header_checksum: [u8; 32],
    /// Total size of all tables, in bytes.
    pub tables_size: u32,
    /// SHA256 checksum of all table contents.
    pub tables_checksum: [u8; 32],
    /// Partition table descriptor.
    pub partitions: MetadataTableDescriptor,
    /// Extent table descriptor.
    pub extents: MetadataTableDescriptor,
    /// Group table descriptor.
    pub groups: MetadataTableDescriptor,
    /// Block device table descriptor.
    pub block_devices: MetadataTableDescriptor,
    /// Flags are informational. See `HEADER_FLAG_*` constants for possible values. This field is
    /// only found in metadata header version 1.2+. New flags may be added without bumping the
    /// version.
    pub flags: MetadataHeaderFlags,
    /// Reserved zero, pad to 256 bytes. This is only included in metadata header version 1.2+.
    pub reserved: [u8; 124],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Immutable, FromBytes, IntoBytes)]
pub struct MetadataHeaderFlags(u32);
bitflags! {
    impl MetadataHeaderFlags: u32 {
        /// This device uses Virtual A/B. Note that on retrofit devices, the expanded header (and
        /// hence this flag) may not be present.
        const VIRTUAL_AB_DEVICE = 0x1;
        /// This device has overlays activated via "adb remount".
        const OVERLAYS_ACTIVE = 0x2;
    }
}

/// Size of the older compatible version of metadata header which includes all but `flags` and
/// `reserved` fields of the current `MetadataHeader`. The metadata header is expected to be this
/// size for version 1.0 and 1.1.
pub const METADATA_HEADER_V1_SIZE: usize = std::mem::size_of::<MetadataHeader>()
    - std::mem::size_of::<u32>()
    - std::mem::size_of::<[u8; 124]>();

impl MetadataHeader {
    pub fn compute_checksum(&self) -> [u8; 32] {
        // The `checksum` field is expected to be zero when computing the SHA256 checksum.
        let mut temp_metadata_header = self.clone();
        temp_metadata_header.header_checksum = [0; 32];
        return if self.minor_version < METADATA_VERSION_FOR_EXPANDED_HEADER_MIN {
            sha2::Sha256::digest(&temp_metadata_header.as_bytes()[..METADATA_HEADER_V1_SIZE]).into()
        } else {
            sha2::Sha256::digest(temp_metadata_header.as_bytes()).into()
        };
    }

    pub fn validate(&mut self) -> Result<(), Error> {
        ensure!(self.magic == METADATA_HEADER_MAGIC, "Invalid metadata header magic.");

        ensure!(self.major_version == METADATA_MAJOR_VERSION, "Incompatible metadata version.");
        ensure!(self.minor_version <= METADATA_MINOR_VERSION_MAX, "Incompatible metadata version.");

        if self.minor_version < METADATA_VERSION_FOR_EXPANDED_HEADER_MIN {
            // Verify size against the old metadata header
            ensure!(
                self.header_size == METADATA_HEADER_V1_SIZE as u32,
                "Incompatible metadata header struct size."
            );
            // If metadata header is the previous version, zero the fields that did not exist.
            self.flags = MetadataHeaderFlags::empty();
            self.reserved = [0; 124];
        } else {
            ensure!(
                self.header_size == std::mem::size_of::<MetadataHeader>() as u32,
                "Incompatible metadata header struct size."
            );
        }

        ensure!(
            self.header_checksum == self.compute_checksum(),
            "Invalid metadata header checksum."
        );

        self.partitions
            .validate(self.tables_size)
            .map_err(|e| anyhow!("partitions tables failed table bounds check: {e}"))?;
        self.extents
            .validate(self.tables_size)
            .map_err(|e| anyhow!("extents tables failed table bounds check: {e}"))?;
        self.groups
            .validate(self.tables_size)
            .map_err(|e| anyhow!("groups tables failed table bounds check: {e}"))?;
        self.block_devices
            .validate(self.tables_size)
            .map_err(|e| anyhow!("block_devices tables failed table bounds check: {e}"))?;

        ensure!(
            self.partitions.entry_size == std::mem::size_of::<MetadataPartition>() as u32,
            "Invalid partition table entry size."
        );
        ensure!(
            self.extents.entry_size == std::mem::size_of::<MetadataExtent>() as u32,
            "Invalid extent table entry size."
        );
        ensure!(
            self.groups.entry_size == std::mem::size_of::<MetadataPartitionGroup>() as u32,
            "Invalid partition group table entry size."
        );
        ensure!(
            self.block_devices.entry_size == std::mem::size_of::<MetadataBlockDevice>() as u32,
            "Invalid block device table entry size."
        );

        Ok(())
    }
}

/// These table descriptors are found in `MetadataHeader`.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, FromBytes, IntoBytes, Immutable)]
pub struct MetadataTableDescriptor {
    /// Location of the table (byte offset relative to the end of the metadata header).
    pub offset: u32,
    /// Number of entries in the table.
    pub num_entries: u32,
    /// Size, in bytes, of each entry in the table. The total size of the table (`num_entries`
    /// multiplied `entry_size`) must not exceed a 32-bit signed integer.
    pub entry_size: u32,
}

impl MetadataTableDescriptor {
    pub fn get_table_size(&self) -> Result<u32, Error> {
        self.num_entries
            .checked_mul(self.entry_size)
            .ok_or_else(|| anyhow!("calculate table size: arithmetic overflow"))
    }

    fn validate(&self, total_tables_size: u32) -> Result<(), Error> {
        ensure!(self.offset < total_tables_size, "Invalid table bounds.");
        ensure!(self.get_table_size()? < total_tables_size, "Invalid table bounds.");
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Immutable, FromBytes, IntoBytes)]
pub struct PartitionAttributes(u32);
bitflags! {
    impl PartitionAttributes: u32 {
        /// This partition is not writable.
        const READONLY = 1 << 0;
        /// If set, indicates that the partition name needs a slot suffix applied. The slot suffix
        /// is determined by the metadata slot number (e.g. slot 0 will have suffix "_a", and slot 1
        /// will have suffix "_b"). This will be applied when the metadata is read. Note that this
        /// is only intended to be used with super_empty.img and super.img on retrofit devices.
        const SLOT_SUFFIXED = 1 << 1;
        /// If set, indicates the the partition was created (or modified) for a snapshot-based
        /// update. If not present, the partition was likely flashed via fastboot.
        const UPDATED = 1 << 2;
        /// If set, indicates that this partition is disabled.
        const DISABLED = 1 << 3;
    }
}

/// The metadata table entries should implement this trait to validate its contents. Useful when
/// parsing the table entries from the super image.
pub trait ValidateTable {
    // Metadata contains four different types of table entries - MetadataPartition, MetadataExtent,
    // MetadataPartitionGroup, and MetadataBlockDevice (see `MetadataHeader`). Call this function to
    // check if the table entries are valid. The checks will be dependent on the type of table
    // entry, for example, `MetadataPartition` table entry has `attributes` and we check that it
    // only contains the allowed attributes.
    fn validate(&self, header: &MetadataHeader) -> Result<(), Error>;
}

/// Adjust names of the slot to include a slot suffix. The suffix attached will be dependent on the
/// slot number. Only two slot numbers are accepted. Slot number 0 will map to suffix "_a" and slot
/// number 1 will map to suffix "_b". The names are adjusted when metadata is read and if the
/// `*::SLOT_SUFFIXED` flag is set. There must be enough characters in the name (total 36) to allow
/// for adding the suffix. On success, unset the `*::SLOT_SUFFIXED` flag.
pub trait AdjustSlotSuffix {
    fn adjust_for_slot_suffix(&mut self, slot_number: u32) -> Result<(), Error>;
}

// Some of the structs contain u8 arrays representing ASCII characters. They may contain trailing
// zeros. This function returns the length of the sub-slice before we encounter the first zero (if
// there are no zeroes, then the returned length will be the size of the original array).
fn get_trimmed_len(chars: &[u8]) -> usize {
    chars.iter().position(|&x| x == 0).unwrap_or(chars.len())
}

// Used to update slot names to include the slot suffix.
fn add_slot_suffix(name: &mut [u8], slot_number: u32) -> Result<(), Error> {
    ensure!(slot_number < 2, "Invalid slot number. Must be 0 or 1.");

    let trimmed_len = get_trimmed_len(&name);
    // This should have been check in `validate()` , but just in case the caller did not
    // call it, check that we can add suffix.
    ensure!(trimmed_len <= name.len() - 2, "Logical partition name is too long");

    name[trimmed_len..trimmed_len + 2].copy_from_slice(if slot_number == 0 {
        b"_a"
    } else {
        b"_b"
    });
    Ok(())
}

pub const PARTITION_ATTRIBUTE_MASK_V0: PartitionAttributes =
    PartitionAttributes::READONLY.union(PartitionAttributes::SLOT_SUFFIXED);
pub const PARTITION_ATTRIBUTE_MASK_V1: PartitionAttributes =
    PartitionAttributes::UPDATED.union(PartitionAttributes::DISABLED);
pub const PARTITION_ATTRIBUTE_MASK: PartitionAttributes =
    PARTITION_ATTRIBUTE_MASK_V0.union(PARTITION_ATTRIBUTE_MASK_V1);

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MetadataPartition {
    /// Name of this partition in ASCII characters. Unused characters in the buffer are set to zero.
    pub name: [u8; 36],
    pub attributes: PartitionAttributes,
    pub first_extent_index: u32,
    pub num_extents: u32,
    pub group_index: u32,
}

impl MetadataPartition {
    pub fn trimmed_name(&self) -> Result<String, Error> {
        Ok(String::from_utf8(self.name.to_vec())
            .map_err(|e| anyhow!("failed to convert partition UTF8 name to string: {e}"))?
            .trim_end_matches(|c| c == '\0')
            .to_string())
    }
}

impl ValidateTable for MetadataPartition {
    fn validate(&self, header: &MetadataHeader) -> Result<(), Error> {
        if header.minor_version < METADATA_VERSION_FOR_UPDATED_ATTRIBUTES_MIN {
            ensure!(self.attributes.difference(PARTITION_ATTRIBUTE_MASK_V0).is_empty());
        } else {
            ensure!(self.attributes.difference(PARTITION_ATTRIBUTE_MASK).is_empty());
        }
        ensure!(
            self.first_extent_index + self.num_extents >= self.first_extent_index,
            "Logical partition's first_extent_index and num_extents overflowed."
        );
        ensure!(
            self.first_extent_index + self.num_extents <= header.extents.num_entries,
            "Logical partition has invalid extent list."
        );
        ensure!(
            self.group_index < header.groups.num_entries,
            "Logical partition has invalid group index."
        );
        ensure!(self.name.is_ascii(), "Logical parition name is not ASCII.");
        let _ = self
            .trimmed_name()
            .map_err(|e| anyhow!("Logical partition name canot be converted to String {e}."))?;

        let attributes = self.attributes;
        if attributes.contains(PartitionAttributes::SLOT_SUFFIXED) {
            // If `PartitionAttributes::SLOT_SUFFIXED`, `name` will have a slot suffix applied (
            // either "_a" or "_b" depending on the slot number) when the metadata is read. Ensure
            // that there are enough characters to add the suffix.
            ensure!(
                get_trimmed_len(&self.name) <= self.name.len() - 2,
                "Logical partition name is too long"
            );
        }
        Ok(())
    }
}

impl AdjustSlotSuffix for MetadataPartition {
    fn adjust_for_slot_suffix(&mut self, slot_number: u32) -> Result<(), Error> {
        let attributes = self.attributes;
        if attributes.contains(PartitionAttributes::SLOT_SUFFIXED) {
            add_slot_suffix(&mut self.name, slot_number)?;
            self.attributes = attributes.difference(PartitionAttributes::SLOT_SUFFIXED);
        }
        Ok(())
    }
}

/// If this is set as `target_type`, implies that the extent is a dm-linear target.
pub const TARGET_TYPE_LINEAR: u32 = 0;
/// This extent is a dm-zero target. The index is ignored and must be zero.
pub const TARGET_TYPE_ZERO: u32 = 1;

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MetadataExtent {
    /// Length of the extent, in 512-byte sectors.
    pub num_sectors: u64,
    /// Target type for device-mapper on Linux. See constants `TARGET_TYPE_*` for possible values.
    pub target_type: u32,
    /// If `target_type` is:
    ///   * `TARGET_TYPE_LINEAR`: this is the sector on the physical partition that this extent maps
    ///     onto.
    ///   * `TARGET_TYPE_ZERO`: this must be zero.
    pub target_data: u64,
    /// If `target_type` is:
    ///   * `TARGET_TYPE_LINEAR`: this must be an index into the block devices table.
    ///   * `TARGET_TYPE_ZERO`: this must be zero.
    pub target_source: u32,
}

impl ValidateTable for MetadataExtent {
    fn validate(&self, header: &MetadataHeader) -> Result<(), Error> {
        match self.target_type {
            TARGET_TYPE_LINEAR => {
                ensure!(
                    self.target_source < header.block_devices.num_entries,
                    "Extent has invalid block device."
                );
            }
            TARGET_TYPE_ZERO => {
                ensure!(self.target_data == 0, "Extent has invalid target data.");
                ensure!(self.target_source == 0, "Extent has invalid target source.");
            }
            _ => {
                return Err(anyhow!("Extent has invalid target type."));
            }
        }
        Ok(())
    }
}

impl MetadataExtent {
    pub fn as_byte_range(&self) -> Result<SuperDeviceRange, Error> {
        let start = self
            .target_data
            .checked_mul(SECTOR_SIZE as u64)
            .ok_or_else(|| anyhow!("overflow occured when calculating start range"))?;
        let length = self
            .num_sectors
            .checked_mul(SECTOR_SIZE as u64)
            .ok_or_else(|| anyhow!("overflow occured when calculating length of range"))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| anyhow!("overflow occured when calculating end of range"))?;
        Ok(SuperDeviceRange(start..end))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Immutable, FromBytes, IntoBytes)]
pub struct PartitionGroupFlags(u32);
bitflags! {
    impl PartitionGroupFlags: u32 {
        /// If this is set, indicates that the partition group name needs a slot suffix applied. The
        /// slot suffix is determined by the metadata slot number (e.g. slot 0 will have suffix
        /// "_a", and slot 1 will have suffix "_b"). This will be applied when the metadata is read.
        /// Note that this flag is only intended to be used with super_empty.img and super.img on
        /// retrofit devices.
        const SLOT_SUFFIXED = 1 << 0;
    }
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MetadataPartitionGroup {
    /// Name of this group in ASCII characters. Unused characters in the buffer are set to zero.
    pub name: [u8; 36],
    pub flags: PartitionGroupFlags,
    /// Maximum size in bytes. If 0, indicates that this group has no maximum size.
    pub maximum_size: u64,
}

impl ValidateTable for MetadataPartitionGroup {
    fn validate(&self, _header: &MetadataHeader) -> Result<(), Error> {
        ensure!(self.name.is_ascii(), "Partition group name is not ASCII.");
        let flags = self.flags;
        if flags.contains(PartitionGroupFlags::SLOT_SUFFIXED) {
            // If `PartitionGroupFlags::SLOT_SUFFIXED`, `name` will have a slot suffix applied (
            // either "_a" or "_b" depending on the slot number) when the metadata is read. Ensure
            // that there are enough characters to add the suffix.
            ensure!(
                get_trimmed_len(&self.name) <= self.name.len() - 2,
                "Partition group name is too long"
            );
        }
        Ok(())
    }
}

impl AdjustSlotSuffix for MetadataPartitionGroup {
    fn adjust_for_slot_suffix(&mut self, slot_number: u32) -> Result<(), Error> {
        let flags = self.flags;
        if flags.contains(PartitionGroupFlags::SLOT_SUFFIXED) {
            add_slot_suffix(&mut self.name, slot_number)?;
            self.flags = flags.difference(PartitionGroupFlags::SLOT_SUFFIXED);
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Immutable, FromBytes, IntoBytes, KnownLayout)]
pub struct BlockDeviceFlags(u32);
bitflags! {
    impl BlockDeviceFlags: u32 {
        /// Similar to the other `*_Flags::SLOT_SUFFIXED` flag. If this is set, then the
        /// `partition_name` in block device needs the slot suffix applied. The slot suffix is
        /// determined by the metadata slot number (e.g. 0 => "_a" and 1 => "_b"). This will be
        /// applied when the metadata is read.  Note that this flag is only intended to be used with
        /// super_empty.img and super.img on retrofit devices.
        const SLOT_SUFFIXED = 1 << 0;
    }
}

/// Defines an entry in the `block_device` table. There must be at least one device and the first
/// device must represent the partition holding the super metadata.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MetadataBlockDevice {
    /// First usable sector for allocating logical partitions. This is the first sector after the
    /// metadata region (consists of the the geometry blocks, and space consumed by the metadata
    /// and the metadata backup).
    pub first_logical_sector: u64,
    /// Alignment for defining partitions or partition extents. For example, an alignment of 1MiB
    /// will require that all partitions have a size evenly divisible by 1MiB, and that the smallest
    /// unit the partition can grow by is 1MiB.
    ///
    /// Alignment is normally determined at runtime when growing or adding partitions. If for some
    /// reason the alignment cannot be determined, then this predefined alignment in the geometry is
    /// used instead. By default it is set to 1MiB.
    pub alignment: u32,
    /// Alignment offset for "stacked" devices. For example, if the "super" partition is not aligned
    /// within the parent block device's partition table, then this is used in deciding where to
    /// place `first_logical_sector`.
    ///
    /// Similar to `alignment`, this will be derived from the operating system. If it cannot be
    /// determined, it is assumed to be zero.
    pub alignment_offset: u32,
    /// Block device size in bytes, as specified when the metadata was created. This can be used to
    /// verify the geometry against a target device.
    pub size: u64,
    /// Partition name in the GPT. Unused characters must be zero.
    pub partition_name: [u8; 36],
    pub flags: BlockDeviceFlags,
}

impl ValidateTable for MetadataBlockDevice {
    fn validate(&self, _header: &MetadataHeader) -> Result<(), Error> {
        let _ = self
            .get_first_logical_sector_in_bytes()
            .map_err(|e| anyhow!("Invalid first logical sector: {e}"))?;
        ensure!(self.partition_name.is_ascii(), "Block device name is not ASCII.");
        let flags = self.flags;
        if flags.contains(BlockDeviceFlags::SLOT_SUFFIXED) {
            // If `BlockDeviceFlags::SLOT_SUFFIXED`, `name` will have a slot suffix applied (
            // either "_a" or "_b" depending on the slot number) when the metadata is read. Ensure
            // that there are enough characters to add the suffix.
            ensure!(
                get_trimmed_len(&self.partition_name) <= self.partition_name.len() - 2,
                "Block device name is too long"
            );
        }
        Ok(())
    }
}

impl AdjustSlotSuffix for MetadataBlockDevice {
    fn adjust_for_slot_suffix(&mut self, slot_number: u32) -> Result<(), Error> {
        let flags = self.flags;
        if flags.contains(BlockDeviceFlags::SLOT_SUFFIXED) {
            add_slot_suffix(&mut self.partition_name, slot_number)?;
            self.flags = flags.difference(BlockDeviceFlags::SLOT_SUFFIXED);
        }
        Ok(())
    }
}

impl MetadataBlockDevice {
    /// Returns offset of the first logical sector in bytes.
    pub fn get_first_logical_sector_in_bytes(&self) -> Result<u64, Error> {
        self.first_logical_sector
            .checked_mul(SECTOR_SIZE.into())
            .ok_or_else(|| anyhow!("calculate first logical sector bytes: arithmetic overflow"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SuperDeviceRange;

    const VALID_METADATA_GEOMETRY_BEFORE_COMPUTING_CHECKSUM: MetadataGeometry = MetadataGeometry {
        magic: METADATA_GEOMETRY_MAGIC,
        struct_size: std::mem::size_of::<MetadataGeometry>() as u32,
        checksum: [0; 32],
        metadata_max_size: 6 * SECTOR_SIZE,
        metadata_slot_count: 2,
        logical_block_size: 6 * SECTOR_SIZE,
    };

    #[fuchsia::test]
    async fn test_valid_metadata_geometry() {
        let mut geometry = VALID_METADATA_GEOMETRY_BEFORE_COMPUTING_CHECKSUM;

        // Should fail validation before computing checksum.
        geometry.validate().expect_err("metadata geometry passed validation unexpectedly");

        let checksum = geometry.compute_checksum();
        geometry.checksum = checksum;
        geometry.validate().expect("metadata geometry failed validation");
    }

    #[fuchsia::test]
    async fn test_invalid_metadata_geometry_magic() {
        let mut geometry = VALID_METADATA_GEOMETRY_BEFORE_COMPUTING_CHECKSUM;
        geometry.magic = 1;
        let checksum = geometry.compute_checksum();
        geometry.checksum = checksum;
        geometry.validate().expect_err("metadata geometry passed validation unexpectedly");
    }

    #[fuchsia::test]
    async fn test_invalid_metadata_geometry_struct_size() {
        let mut geometry = VALID_METADATA_GEOMETRY_BEFORE_COMPUTING_CHECKSUM;
        geometry.struct_size = 8192;
        let checksum = geometry.compute_checksum();
        geometry.checksum = checksum;
        geometry.validate().expect_err("metadata geometry passed validation unexpectedly");
    }

    #[fuchsia::test]
    async fn test_invalid_metadata_geometry_metadata_max_size() {
        let mut geometry = VALID_METADATA_GEOMETRY_BEFORE_COMPUTING_CHECKSUM;
        geometry.metadata_max_size = geometry.metadata_max_size - 1;
        let checksum = geometry.compute_checksum();
        geometry.checksum = checksum;
        geometry.validate().expect_err("metadata geometry passed validation unexpectedly");
    }

    #[fuchsia::test]
    async fn test_invalid_metadata_geometry_metadata_slot_count() {
        let mut geometry = VALID_METADATA_GEOMETRY_BEFORE_COMPUTING_CHECKSUM;
        geometry.metadata_slot_count = 0;
        let checksum = geometry.compute_checksum();
        geometry.checksum = checksum;
        geometry.validate().expect_err("metadata geometry passed validation unexpectedly");
    }

    #[fuchsia::test]
    async fn test_invalid_metadata_geometry_logical_block_size() {
        let mut geometry = VALID_METADATA_GEOMETRY_BEFORE_COMPUTING_CHECKSUM;
        geometry.logical_block_size = geometry.logical_block_size - 1;
        let checksum = geometry.compute_checksum();
        geometry.checksum = checksum;
        geometry.validate().expect_err("metadata geometry passed validation unexpectedly");
    }

    #[fuchsia::test]
    async fn test_get_metadata_offset() {
        let mut geometry = VALID_METADATA_GEOMETRY_BEFORE_COMPUTING_CHECKSUM;
        geometry.checksum = geometry.compute_checksum();
        geometry.validate().expect("metadata geometry failed validation");

        let primary_offset_slot_a =
            geometry.get_primary_metadata_offset(0).expect("failed to get primary metadata offset");
        assert_eq!(primary_offset_slot_a, RESERVED_AND_GEOMETRIES_SIZE);
        let primary_offset_slot_b =
            geometry.get_primary_metadata_offset(1).expect("failed to get primary metadata offset");
        assert_eq!(
            primary_offset_slot_b,
            RESERVED_AND_GEOMETRIES_SIZE + geometry.metadata_max_size as u64
        );

        let backup_offset_slot_a =
            geometry.get_backup_metadata_offset(0).expect("failed to get backup metadata offset");
        assert_eq!(
            backup_offset_slot_a,
            RESERVED_AND_GEOMETRIES_SIZE + 2 * geometry.metadata_max_size as u64
        );
        let backup_offset_slot_b =
            geometry.get_backup_metadata_offset(1).expect("failed to get backup metadata offset");
        assert_eq!(
            backup_offset_slot_b,
            RESERVED_AND_GEOMETRIES_SIZE + 3 * geometry.metadata_max_size as u64
        );
    }

    const VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM: MetadataHeader = MetadataHeader {
        magic: METADATA_HEADER_MAGIC,
        major_version: METADATA_MAJOR_VERSION,
        minor_version: METADATA_VERSION_FOR_EXPANDED_HEADER_MIN,
        header_size: std::mem::size_of::<MetadataHeader>() as u32,
        header_checksum: [0; 32],
        tables_size: 188,
        tables_checksum: [0; 32],
        partitions: MetadataTableDescriptor { offset: 0, num_entries: 1, entry_size: 52 },
        extents: MetadataTableDescriptor { offset: 52, num_entries: 1, entry_size: 24 },
        groups: MetadataTableDescriptor { offset: 76, num_entries: 1, entry_size: 48 },
        block_devices: MetadataTableDescriptor { offset: 124, num_entries: 1, entry_size: 64 },
        flags: MetadataHeaderFlags::VIRTUAL_AB_DEVICE,
        reserved: [0; 124],
    };

    #[fuchsia::test]
    async fn test_valid_metadata_header() {
        let mut header = VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM;

        // Should fail validation before computing checksum.
        header.validate().expect_err("metadata header passed validation unexpectedly");

        let checksum = header.compute_checksum();
        header.header_checksum = checksum;
        header.validate().expect("metadata header failed validation");
    }

    #[fuchsia::test]
    async fn test_valid_older_metadata_header() {
        let mut header = VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM;
        header.minor_version = 1;
        assert!(header.minor_version < METADATA_VERSION_FOR_EXPANDED_HEADER_MIN);
        header.header_size = METADATA_HEADER_V1_SIZE as u32;

        // Zero the fields that does not exist in the older version of the metadata header before
        // calculating its checksum.
        header.flags = MetadataHeaderFlags::empty();
        header.reserved = [0; 124];
        let checksum = header.compute_checksum();
        header.header_checksum = checksum;
        header.validate().expect("metadata header failed validation");
    }

    #[fuchsia::test]
    async fn test_invalid_metadata_header() {
        let mut valid_header = VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM;
        let checksum = valid_header.compute_checksum();
        valid_header.header_checksum = checksum;
        valid_header.validate().expect("metadata header failed validation");

        // Check that there is validation check for the magic.
        {
            let mut invalid_header = valid_header.clone();
            invalid_header.magic = 1;
            invalid_header.header_checksum = invalid_header.compute_checksum();
            invalid_header
                .validate()
                .expect_err("metadata header with invalid magic passed validation unexpectedly");
        }

        // Check that there is validation check for the major version.
        {
            let mut invalid_header = valid_header.clone();
            invalid_header.major_version = 0;
            invalid_header.header_checksum = invalid_header.compute_checksum();
            invalid_header.validate().expect_err(
                "metadata header with invalid major version passed validation unexpectedly",
            );
        }

        // Check that there is validation check for the minor version.
        {
            let mut invalid_header = valid_header.clone();
            invalid_header.minor_version = METADATA_MINOR_VERSION_MAX + 1;
            invalid_header.header_checksum = invalid_header.compute_checksum();
            invalid_header.validate().expect_err(
                "metadata header with invalid minor version passed validation unexpectedly",
            );
        }

        // Check that there is validation check for the header struct size.
        {
            let mut invalid_header = valid_header.clone();
            invalid_header.header_size = 0;
            invalid_header.header_checksum = invalid_header.compute_checksum();
            invalid_header.validate().expect_err(
                "metadata header with invalid header size passed validation unexpectedly",
            );
        }

        // Check that there is validation check for the table bounds.
        {
            let mut invalid_header = valid_header.clone();
            invalid_header.tables_size = 0;
            invalid_header.header_checksum = invalid_header.compute_checksum();
            invalid_header.validate().expect_err(
                "metadata header with invalid tables size passed validation unexpectedly",
            );
        }

        // Check that there is validation check for the table bounds.
        {
            let mut invalid_header = valid_header.clone();
            invalid_header.block_devices.offset *= 2;
            invalid_header.header_checksum = invalid_header.compute_checksum();
            invalid_header.validate().expect_err(
                "metadata header with invalid entry offset passed validation unexpectedly",
            );
        }

        // Check that there is validation check for the table size overflow.
        {
            let mut invalid_header = valid_header.clone();
            invalid_header.block_devices.num_entries = u32::MAX;
            invalid_header.header_checksum = invalid_header.compute_checksum();
            invalid_header.validate().expect_err(
                "metadata header with invalid num_entries passed validation unexpectedly",
            );
        }
    }

    const VALID_PARTITION_TABLE_ENTRY: MetadataPartition = MetadataPartition {
        name: [
            115, 121, 115, 116, 101, 109, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        attributes: PartitionAttributes::READONLY,
        first_extent_index: 0,
        num_extents: 1,
        group_index: 0,
    };

    #[fuchsia::test]
    async fn test_valid_partition_table_entry() {
        let partition = VALID_PARTITION_TABLE_ENTRY;
        partition
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect("metadata partition failed validation");
    }

    #[fuchsia::test]
    async fn test_partition_table_entry_with_slot_suffixed() {
        let mut partition = VALID_PARTITION_TABLE_ENTRY;
        partition.attributes = PartitionAttributes::SLOT_SUFFIXED;
        partition
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect("metadata partition failed validation");

        let original_name = String::from_utf8(partition.name.to_vec())
            .expect("failed to convert partition name to string");

        for (num, suffix) in [(0, "_a"), (1, "_b")] {
            let mut partition = partition.clone();
            partition.adjust_for_slot_suffix(num).expect("adjust for slot suffix failed");
            let expected_name =
                format!("{}{}", original_name.clone().trim_end_matches(|c| c == '\0'), suffix);
            let updated_name = String::from_utf8(partition.name.to_vec())
                .expect("failed to convert partition name to string");

            assert_eq!(updated_name.trim_end_matches(|c| c == '\0'), expected_name);
            let attributes = partition.attributes;
            assert!(attributes.is_empty());
        }
    }

    #[fuchsia::test]
    async fn test_invalid_partition_table_entry() {
        let header = VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM;

        // Check that there is validation check for extent list.
        {
            let mut invalid_partition = VALID_PARTITION_TABLE_ENTRY;
            invalid_partition.first_extent_index = 10;
            assert!(header.partitions.num_entries < invalid_partition.first_extent_index);
            invalid_partition
                .validate(&header)
                .expect_err("metadata partition passed validation unexpectedly");
        }

        // Check that there is validation check for group index.
        {
            let mut invalid_partition = VALID_PARTITION_TABLE_ENTRY;
            invalid_partition.group_index = 8;
            assert!(header.groups.num_entries < invalid_partition.group_index);
            invalid_partition
                .validate(&header)
                .expect_err("metadata partition passed validation unexpectedly");
        }

        // Check that there is validation check for partition attributes.
        {
            let mut invalid_partition = VALID_PARTITION_TABLE_ENTRY;
            let mut invalid_header = VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM;
            invalid_header.minor_version = 0;
            assert!(invalid_header.minor_version < METADATA_VERSION_FOR_UPDATED_ATTRIBUTES_MIN);
            invalid_partition.attributes = PARTITION_ATTRIBUTE_MASK_V1;
            invalid_partition
                .validate(&invalid_header)
                .expect_err("metadata partition passed validation unexpectedly");
        }

        // Check that there is validation check for partition name.
        {
            let mut invalid_partition = VALID_PARTITION_TABLE_ENTRY;
            invalid_partition.name[0] = 0x81;
            assert!(!invalid_partition.name[0].is_ascii());
            invalid_partition
                .validate(&header)
                .expect_err("metadata partition passed validation unexpectedly");
        }
    }

    #[fuchsia::test]
    async fn test_partition_trimmed_name() {
        let partition = VALID_PARTITION_TABLE_ENTRY;
        partition
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect("metadata partition failed validation");
        let partition_name =
            partition.trimmed_name().expect("MetadataPartition::trimmed_name failed");
        assert_eq!(partition_name, "system");
    }

    const VALID_EXTENT: MetadataExtent = MetadataExtent {
        num_sectors: 4,
        target_type: TARGET_TYPE_LINEAR,
        target_data: 1,
        target_source: 0,
    };

    #[fuchsia::test]
    async fn test_valid_extent() {
        let extent = VALID_EXTENT;
        extent
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect("metadata extent failed validation");
    }

    #[fuchsia::test]
    async fn test_invalid_extent() {
        // Check that target_source is validated
        {
            let mut invalid_extent = VALID_EXTENT;
            invalid_extent.target_source = 10;
            assert!(
                invalid_extent.target_source
                    >= VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM.block_devices.num_entries
            );
            invalid_extent
                .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
                .expect_err("metadata extent passed validation unexpectedly");
        }

        // Check that TARGET_TYPE_ZERO must have target_data and target_source as zero.
        {
            let mut invalid_extent = VALID_EXTENT;
            invalid_extent.target_type = TARGET_TYPE_ZERO;
            invalid_extent.target_data = 1;
            invalid_extent
                .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
                .expect_err("metadata extent passed validation unexpectedly");
        }
        {
            let mut invalid_extent = VALID_EXTENT;
            invalid_extent.target_type = TARGET_TYPE_ZERO;
            invalid_extent.target_source = 1;
            invalid_extent
                .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
                .expect_err("metadata extent passed validation unexpectedly");
        }
    }

    #[fuchsia::test]
    async fn test_extent_as_range() {
        let extent = MetadataExtent {
            num_sectors: 3,
            target_type: TARGET_TYPE_LINEAR,
            target_data: 1,
            target_source: 0,
        };
        let range = extent.as_byte_range().expect("failed to convert metadata extent to range");
        let start = 1 * SECTOR_SIZE as u64;
        let end = start + 3 * SECTOR_SIZE as u64;
        assert_eq!(range, SuperDeviceRange(start..end));
    }

    #[fuchsia::test]
    async fn test_extent_as_range_overflow() {
        let extent = MetadataExtent {
            num_sectors: u64::MAX,
            target_type: TARGET_TYPE_LINEAR,
            target_data: 1,
            target_source: 0,
        };
        extent.as_byte_range().expect_err("converting extent to range should have failed");
    }

    const VALID_PARTITION_GROUP: MetadataPartitionGroup = MetadataPartitionGroup {
        name: [
            115, 121, 115, 116, 101, 109, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        flags: PartitionGroupFlags(0),
        maximum_size: 0,
    };

    #[fuchsia::test]
    async fn test_valid_partition_group() {
        let group = VALID_PARTITION_GROUP;
        group
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect("metadata partition group failed validation");
    }

    #[fuchsia::test]
    async fn test_partition_group_with_slot_suffixed() {
        let mut group = VALID_PARTITION_GROUP;
        group.flags = PartitionGroupFlags::SLOT_SUFFIXED;
        group
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect("metadata partition group failed validation");

        let original_group_name = String::from_utf8(group.name.to_vec())
            .expect("failed to convert partition name to string");

        for (num, suffix) in [(0, "_a"), (1, "_b")] {
            let mut group = group.clone();
            group.adjust_for_slot_suffix(num).expect("adjust for slot suffix failed");
            let expected_name = format!(
                "{}{}",
                original_group_name.clone().trim_end_matches(|c| c == '\0'),
                suffix
            );
            let updated_name = String::from_utf8(group.name.to_vec())
                .expect("failed to convert partition name to string");

            assert_eq!(updated_name.trim_end_matches(|c| c == '\0'), expected_name);
            let flags = group.flags;
            assert!(flags.is_empty());
        }
    }

    #[fuchsia::test]
    async fn test_invalid_partition_group() {
        let mut invalid_group = VALID_PARTITION_GROUP;
        invalid_group.name[0] = 0x81;
        assert!(!invalid_group.name[0].is_ascii());
        invalid_group
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect_err("metadata partition group failed validation");
    }

    const VALID_METADATA_BLOCK_DEVICE: MetadataBlockDevice = MetadataBlockDevice {
        first_logical_sector: 2048,
        alignment: 1048576,
        alignment_offset: 0,
        size: 1073741824,
        partition_name: [
            115, 117, 112, 101, 114, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        flags: BlockDeviceFlags(0),
    };

    #[fuchsia::test]
    async fn test_valid_block_device() {
        let metadata_block_device = VALID_METADATA_BLOCK_DEVICE;
        metadata_block_device
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect("metadata block device failed validation");
    }

    #[fuchsia::test]
    async fn test_block_device_with_slot_suffixed() {
        let mut metadata_block_device = VALID_METADATA_BLOCK_DEVICE;
        metadata_block_device.flags = BlockDeviceFlags::SLOT_SUFFIXED;
        metadata_block_device
            .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
            .expect("metadata block device failed validation");

        let original_partition_name =
            String::from_utf8(metadata_block_device.partition_name.to_vec())
                .expect("failed to convert partition name to string");

        for (num, suffix) in [(0, "_a"), (1, "_b")] {
            let mut metadata_block_device = metadata_block_device.clone();
            metadata_block_device
                .adjust_for_slot_suffix(num)
                .expect("adjust for slot suffix failed");
            let expected_name = format!(
                "{}{}",
                original_partition_name.clone().trim_end_matches(|c| c == '\0'),
                suffix
            );
            let updated_partition_name =
                String::from_utf8(metadata_block_device.partition_name.to_vec())
                    .expect("failed to convert partition name to string");

            assert_eq!(updated_partition_name.trim_end_matches(|c| c == '\0'), expected_name);
            let flags = metadata_block_device.flags;
            assert!(flags.is_empty());
        }
    }

    #[fuchsia::test]
    async fn test_invalid_block_device() {
        // Check that first logical sector can be converted to bytes (used to verify that the
        // logical partition contents don't overlap with the metadata region) without an overflow.
        {
            let mut invalid_metadata_block_device = VALID_METADATA_BLOCK_DEVICE;
            invalid_metadata_block_device.first_logical_sector = u64::MAX;
            invalid_metadata_block_device
                .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
                .expect_err("metadata block device passed validation unexpectedly");
        }

        // Check that there is validation check for block device partition name.
        {
            let mut invalid_metadata_block_device = VALID_METADATA_BLOCK_DEVICE;
            invalid_metadata_block_device.partition_name[0] = 0x81;
            assert!(!invalid_metadata_block_device.partition_name[0].is_ascii());
            invalid_metadata_block_device
                .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
                .expect_err("metadata block device passed validation unexpectedly");
        }

        // Check that there is validation check for enough characters in name to add suffix.
        {
            let mut invalid_metadata_block_device = VALID_METADATA_BLOCK_DEVICE;
            invalid_metadata_block_device.partition_name = [b'a'; 36];
            invalid_metadata_block_device.flags = BlockDeviceFlags::SLOT_SUFFIXED;
            invalid_metadata_block_device
                .validate(&VALID_METADATA_HEADER_BEFORE_COMPUTING_CHECKSUM)
                .expect_err("metadata block device passed validation unexpectedly");
        }
    }
}
