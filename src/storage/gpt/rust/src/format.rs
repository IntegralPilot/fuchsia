// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow, ensure};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

const MAX_PARTITION_ENTRIES: u32 = 128;

pub const GPT_SIGNATURE: [u8; 8] = [0x45, 0x46, 0x49, 0x20, 0x50, 0x41, 0x52, 0x54];
pub const GPT_REVISION: u32 = 0x10000;
pub const GPT_HEADER_SIZE: usize = 92;

/// GPT disk header.
#[derive(Clone, Debug, Eq, PartialEq, Immutable, IntoBytes, KnownLayout, FromBytes)]
#[repr(C)]
pub struct Header {
    /// Must be GPT_SIGNATURE
    pub signature: [u8; 8],
    /// Must be GPT_REVISION
    pub revision: u32,
    /// Must be GPT_HEADER_SIZE
    pub header_size: u32,
    /// CRC32 of the header with crc32 section zeroed
    pub crc32: u32,
    /// reserved; must be 0
    pub reserved: u32,
    /// Must be 1
    pub current_lba: u64,
    /// LBA of backup header
    pub backup_lba: u64,
    /// First usable LBA for partitions (primary table last LBA + 1)
    pub first_usable: u64,
    /// Last usable LBA (secondary partition table first LBA - 1)
    pub last_usable: u64,
    /// UUID of the disk
    pub disk_guid: [u8; 16],
    /// Starting LBA of partition entries
    pub part_start: u64,
    /// Number of partition entries
    pub num_parts: u32,
    /// Size of a partition entry, usually 128
    pub part_size: u32,
    /// CRC32 of the partition table
    pub crc32_parts: u32,
    /// Padding to satisfy zerocopy's alignment requirements.
    /// Not actually part of the header, which should be GPT_HEADER_SIZE bytes.
    zerocopy_padding: u32,
}

impl Header {
    pub fn new(block_count: u64, block_size: u32, num_parts: u32) -> Result<Self, Error> {
        ensure!(block_size > 0 && block_size.is_power_of_two(), "Invalid block size");
        let bs = block_size as u64;

        let part_size = std::mem::size_of::<PartitionTableEntry>();
        let partition_table_len = num_parts as u64 * part_size as u64;
        let partition_table_blocks = partition_table_len.checked_next_multiple_of(bs).unwrap() / bs;

        // Ensure there are enough blocks for both copies of the metadata, plus the protective MBR
        // block.
        ensure!(block_count > 1 + 2 * (1 + partition_table_blocks), "Too few blocks");

        let mut this = Self {
            signature: GPT_SIGNATURE,
            revision: GPT_REVISION,
            header_size: GPT_HEADER_SIZE as u32,
            crc32: 0,
            reserved: 0,
            current_lba: 1,
            backup_lba: block_count - 1,
            first_usable: 2 + partition_table_blocks,
            last_usable: block_count - (2 + partition_table_blocks),
            disk_guid: uuid::Uuid::new_v4().into_bytes(),
            part_start: 2,
            num_parts,
            part_size: part_size as u32,
            crc32_parts: 0,
            zerocopy_padding: 0,
        };
        this.update_checksum();
        Ok(this)
    }

    // NB: This is expensive as it deeply copies the header.
    pub fn compute_checksum(&self) -> u32 {
        let mut header_copy = self.clone();
        header_copy.crc32 = 0;
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC)
            .checksum(&header_copy.as_bytes()[..GPT_HEADER_SIZE])
    }

    fn update_checksum(&mut self) {
        self.crc32 = 0;
        let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC)
            .checksum(&self.as_bytes()[..GPT_HEADER_SIZE]);
        self.crc32 = crc;
    }

    // NB: This does *not* validate the partition table checksum.
    pub fn ensure_integrity(&self, block_count: u64, block_size: u64) -> Result<(), Error> {
        ensure!(self.signature == GPT_SIGNATURE, "Bad signature {:x?}", self.signature);
        ensure!(self.revision == GPT_REVISION, "Bad revision {:x}", self.revision);
        ensure!(
            self.header_size as usize == GPT_HEADER_SIZE,
            "Bad header size {}",
            self.header_size
        );

        // Now that we've checked the basic fields, check the CRC.  All other checks should be below
        // this.
        ensure!(self.crc32 == self.compute_checksum(), "Invalid header checksum");

        ensure!(self.num_parts <= MAX_PARTITION_ENTRIES, "Invalid num_parts {}", self.num_parts);
        ensure!(
            self.part_size as usize == std::mem::size_of::<PartitionTableEntry>(),
            "Invalid part_size {}",
            self.part_size
        );
        let partition_table_blocks = (self
            .num_parts
            .checked_mul(self.part_size)
            .and_then(|v| v.checked_next_multiple_of(block_size as u32))
            .ok_or_else(|| {
                anyhow!(
                    "Partition table size overflow \
                     (num_parts: {}, part_size: {}, block_size: {block_size})",
                    self.num_parts,
                    self.part_size
                )
            })? as u64)
            / block_size;
        ensure!(
            partition_table_blocks < block_count,
            "Invalid partition table size: \
             {partition_table_blocks} blocks >= {block_count} block_count"
        );

        // NB: The current LBA points to *this* header, so it's either at the start or the end.
        // The last LBA points to the *other* header.  Since we want to check the absolute offsets,
        // figure out which is which.
        ensure!(
            self.current_lba == 1 || self.current_lba == block_count - 1,
            "Invalid current_lba {}",
            self.current_lba
        );
        if self.current_lba == 1 {
            // We tolerate a backup_lba which is lesser than block_count, since a resizable disk
            // might have grown past the backup header.  This is arguably an invalid GPT (the backup
            // header can no longer be found for recovery), but it happens in practice in some
            // cases, such as an emulator which grows its disk.
            ensure!(
                self.backup_lba < block_count,
                "backup_lba out of bounds {} (block_count {block_count})",
                self.backup_lba,
            );
        } else {
            ensure!(self.backup_lba == 1, "Invalid backup_lba {}", self.backup_lba);
        }
        let (first_lba, second_lba) = if self.current_lba == 1 {
            (self.current_lba, self.backup_lba)
        } else {
            (self.backup_lba, self.current_lba)
        };

        let min_first_usable = first_lba
            .checked_add(1)
            .and_then(|v| v.checked_add(partition_table_blocks))
            .ok_or_else(|| {
                anyhow!(
                    "Overflow calculating min_first_usable \
                     (first_lba: {first_lba}, partition_table_blocks: {partition_table_blocks})"
                )
            })?;
        ensure!(
            self.first_usable >= min_first_usable,
            "Invalid first_usable {} (minimum: {})",
            self.first_usable,
            min_first_usable
        );
        let last_usable_end =
            self.last_usable.checked_add(partition_table_blocks).ok_or_else(|| {
                anyhow!(
                    "Overflow calculating last_usable_end \
                     (last_usable: {}, partition_table_blocks: {partition_table_blocks})",
                    self.last_usable
                )
            })?;
        ensure!(
            self.first_usable <= self.last_usable && last_usable_end < second_lba,
            "Invalid last_usable {} (first_usable: {}, last_usable_end: {}, second_lba: {})",
            self.last_usable,
            self.first_usable,
            last_usable_end,
            second_lba
        );

        if first_lba == self.current_lba {
            ensure!(self.part_start == first_lba + 1, "Invalid part_start {}", self.part_start);
        } else {
            let expected_part_start = self.last_usable.checked_add(1).ok_or_else(|| {
                anyhow!(
                    "Overflow calculating expected part_start (last_usable: {})",
                    self.last_usable
                )
            })?;
            ensure!(
                self.part_start == expected_part_start,
                "Invalid part_start {} (expected: {})",
                self.part_start,
                expected_part_start
            );
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Immutable, IntoBytes, KnownLayout, FromBytes)]
#[repr(C)]
pub struct PartitionTableEntry {
    pub type_guid: [u8; 16],
    pub instance_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
    pub flags: u64,
    pub name: [u16; 36],
}

impl PartitionTableEntry {
    pub fn is_empty(&self) -> bool {
        self.as_bytes().iter().all(|b| *b == 0)
    }

    pub fn empty() -> Self {
        Self {
            type_guid: [0u8; 16],
            instance_guid: [0u8; 16],
            first_lba: 0,
            last_lba: 0,
            flags: 0,
            name: [0u16; 36],
        }
    }

    pub fn ensure_integrity(&self, first_usable: u64, last_usable: u64) -> Result<(), Error> {
        ensure!(self.type_guid != [0u8; 16], "Empty type GUID");
        ensure!(self.instance_guid != [0u8; 16], "Empty instance GUID");
        ensure!(
            self.first_lba >= first_usable,
            "Invalid first LBA {} (first_usable: {})",
            self.first_lba,
            first_usable
        );
        ensure!(
            self.last_lba <= last_usable && self.last_lba >= self.first_lba,
            "Invalid last LBA {} (first_usable: {}, last_usable: {}, first_lba: {})",
            self.last_lba,
            first_usable,
            last_usable,
            self.first_lba
        );
        Ok(())
    }
}

#[derive(Eq, thiserror::Error, Clone, Debug, PartialEq)]
pub enum FormatError {
    #[error("Invalid arguments")]
    InvalidArguments,
    #[error("No space")]
    NoSpace,
}

/// Serializes the partition table, and updates `header` to reflect the changes (including computing
/// the CRC).  Returns the raw bytes of the partition table.
/// Fails if any of the entries in `entries` are invalid.
pub fn serialize_partition_table(
    header: &mut Header,
    block_size: usize,
    num_blocks: u64,
    entries: &[PartitionTableEntry],
) -> Result<Vec<u8>, FormatError> {
    let crc_algo = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);
    let mut digest = crc_algo.digest();
    let partition_table_len = header.part_size as usize * entries.len();
    let partition_table_len = partition_table_len
        .checked_next_multiple_of(block_size)
        .ok_or(FormatError::InvalidArguments)?;
    let partition_table_blocks = (partition_table_len / block_size) as u64;
    let mut partition_table = vec![0u8; partition_table_len];
    let mut partition_table_view = &mut partition_table[..];
    // The first two blocks are resered for the PMBR and the primary GPT header.
    let first_usable = partition_table_blocks + 2;
    // The last block is reserved for the backup GPT header.  We subtract one more to get to the
    // offset of the last usable block.
    let last_usable = num_blocks.saturating_sub(partition_table_blocks + 2);
    if first_usable > last_usable {
        return Err(FormatError::NoSpace);
    }
    let last_usable_end = last_usable.checked_add(1).ok_or(FormatError::InvalidArguments)?;
    let mut used_ranges = vec![0..first_usable, last_usable_end..num_blocks];
    let part_size = header.part_size as usize;
    for entry in entries {
        let part_raw = entry.as_bytes();
        assert!(part_raw.len() == part_size);
        if !entry.is_empty() {
            entry
                .ensure_integrity(first_usable, last_usable)
                .map_err(|_| FormatError::InvalidArguments)?;
            let end = entry.last_lba.checked_add(1).ok_or(FormatError::InvalidArguments)?;
            used_ranges.push(entry.first_lba..end);
            partition_table_view[..part_raw.len()].copy_from_slice(part_raw);
        }
        digest.update(part_raw);
        partition_table_view = &mut partition_table_view[part_size..];
    }
    used_ranges.sort_by_key(|range| range.start);
    for [a, b] in used_ranges.array_windows() {
        if a.end > b.start {
            return Err(FormatError::InvalidArguments);
        }
    }
    header.first_usable = first_usable;
    header.last_usable = last_usable;
    header.num_parts = entries.len() as u32;
    header.crc32_parts = digest.finalize();
    header.crc32 = header.compute_checksum();
    Ok(partition_table)
}

#[cfg(test)]
mod tests {
    use super::{
        FormatError, GPT_HEADER_SIZE, Header, PartitionTableEntry, serialize_partition_table,
    };

    #[fuchsia::test]
    fn header_crc() {
        let nblocks = 8;
        let partition_table_nblocks = 1;
        let mut header = Header {
            signature: [0x45, 0x46, 0x49, 0x20, 0x50, 0x41, 0x52, 0x54],
            revision: 0x10000,
            header_size: GPT_HEADER_SIZE as u32,
            crc32: 0,
            reserved: 0,
            current_lba: 1,
            backup_lba: nblocks - 1,
            first_usable: 2 + partition_table_nblocks,
            last_usable: nblocks - (2 + partition_table_nblocks),
            disk_guid: [0u8; 16],
            part_start: 2,
            num_parts: 1,
            part_size: 128,
            crc32_parts: 0,
            zerocopy_padding: 0,
        };
        header.crc32 = header.compute_checksum();

        header.ensure_integrity(nblocks, 512).expect("Header should be valid");

        // Flip one bit, leaving an otherwise valid header.
        header.num_parts = 2;

        header.ensure_integrity(nblocks, 512).expect_err("Header should be invalid");
    }

    #[fuchsia::test]
    fn test_backup_lba_validation() {
        let nblocks = 10;
        let partition_table_nblocks = 1;
        let mut header = Header {
            signature: [0x45, 0x46, 0x49, 0x20, 0x50, 0x41, 0x52, 0x54],
            revision: 0x10000,
            header_size: GPT_HEADER_SIZE as u32,
            crc32: 0,
            reserved: 0,
            current_lba: 1,
            backup_lba: nblocks - 1,
            first_usable: 2 + partition_table_nblocks,
            last_usable: nblocks - (2 + partition_table_nblocks),
            disk_guid: [0u8; 16],
            part_start: 2,
            num_parts: 1,
            part_size: 128,
            crc32_parts: 0,
            zerocopy_padding: 0,
        };

        // 1. Primary header (current_lba == 1):
        // backup_lba == nblocks - 1 should be valid.
        header.crc32 = header.compute_checksum();
        header.ensure_integrity(nblocks, 512).expect("Header should be valid");

        // backup_lba < nblocks - 1 should be valid.
        header.backup_lba = nblocks - 2;
        header.last_usable = 6;
        header.crc32 = header.compute_checksum();
        header
            .ensure_integrity(nblocks, 512)
            .expect("Header should be valid with relaxed backup_lba");

        // backup_lba >= nblocks should be invalid.
        header.backup_lba = nblocks;
        header.last_usable = nblocks - (2 + partition_table_nblocks);
        header.crc32 = header.compute_checksum();
        header.ensure_integrity(nblocks, 512).expect_err("backup_lba >= nblocks should be invalid");

        // 2. Backup header (current_lba == nblocks - 1):
        header.current_lba = nblocks - 1;
        header.backup_lba = 1;
        header.part_start = header.last_usable + 1;
        header.crc32 = header.compute_checksum();
        header
            .ensure_integrity(nblocks, 512)
            .expect("Backup header should be valid with backup_lba == 1");

        header.backup_lba = 2;
        header.crc32 = header.compute_checksum();
        header
            .ensure_integrity(nblocks, 512)
            .expect_err("Backup header should be invalid with backup_lba != 1");
    }

    #[fuchsia::test]
    fn test_header_ensure_integrity_last_usable_overflow() {
        let nblocks = 10;
        let partition_table_nblocks = 1;
        let mut header = Header {
            signature: [0x45, 0x46, 0x49, 0x20, 0x50, 0x41, 0x52, 0x54],
            revision: 0x10000,
            header_size: GPT_HEADER_SIZE as u32,
            crc32: 0,
            reserved: 0,
            current_lba: 1,
            backup_lba: nblocks - 1,
            first_usable: 2 + partition_table_nblocks,
            last_usable: u64::MAX,
            disk_guid: [0u8; 16],
            part_start: 2,
            num_parts: 1,
            part_size: 128,
            crc32_parts: 0,
            zerocopy_padding: 0,
        };
        header.crc32 = header.compute_checksum();
        header
            .ensure_integrity(nblocks, 512)
            .expect_err("last_usable = u64::MAX should fail ensure_integrity");
    }

    #[fuchsia::test]
    fn test_serialize_partition_table_overflow_entry() {
        let block_count = 1024;
        let block_size = 512;
        let mut header = Header::new(block_count, block_size, 128).unwrap();
        let mut entries = vec![PartitionTableEntry::empty(); 128];
        entries[0] = PartitionTableEntry {
            type_guid: [1; 16],
            instance_guid: [1; 16],
            first_lba: u64::MAX,
            last_lba: u64::MAX,
            flags: 0,
            name: [0; 36],
        };
        let result =
            serialize_partition_table(&mut header, block_size as usize, block_count, &entries[..]);
        assert_eq!(result, Err(FormatError::InvalidArguments));
    }
}
