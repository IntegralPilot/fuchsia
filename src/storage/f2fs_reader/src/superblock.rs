// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::checkpoint::MAX_BITMAP_BYTES;
use anyhow::{Error, anyhow, ensure};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub const F2FS_MAGIC: u32 = 0xf2f52010;
// There are two consecutive superblocks, 1kb each.
pub const SUPERBLOCK_OFFSET: u64 = 1024;
// We only support 4kB blocks.
pub const BLOCK_SIZE: usize = 4096;
// We only support 2MB segments.
pub const BLOCKS_PER_SEGMENT: usize = 512;
pub const SEGMENT_SIZE: usize = BLOCK_SIZE * BLOCKS_PER_SEGMENT;

// Simple CRC used to validate data structures.
pub fn f2fs_crc32(mut seed: u32, buf: &[u8]) -> u32 {
    const CRC_POLY: u32 = 0xedb88320;
    for ch in buf {
        seed ^= *ch as u32;
        for _ in 0..8 {
            seed = (seed >> 1) ^ (if (seed & 1) == 1 { CRC_POLY } else { 0 });
        }
    }
    seed
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub struct SuperBlock {
    pub magic: u32,                 // F2FS_MAGIC
    pub major_ver: u16,             // Major Version
    pub minor_ver: u16,             // Minor Version
    pub log_sectorsize: u32,        // log2 sector size in bytes
    pub log_sectors_per_block: u32, // log2 # of sectors per block
    pub log_blocksize: u32,         // log2 block size in bytes
    pub log_blocks_per_seg: u32,    // log2 # of blocks per segment
    pub segs_per_sec: u32,          // # of segments per section
    pub secs_per_zone: u32,         // # of sections per zone
    pub checksum_offset: u32,       // checksum offset in super block
    pub block_count: u64,           // total # of user blocks
    pub section_count: u32,         // total # of sections
    pub segment_count: u32,         // total # of segments
    pub segment_count_ckpt: u32,    // # of segments for checkpoint
    pub segment_count_sit: u32,     // # of segments for SIT
    pub segment_count_nat: u32,     // # of segments for NAT
    pub segment_count_ssa: u32,     // # of segments for SSA
    pub segment_count_main: u32,    // # of segments for main area
    pub segment0_blkaddr: u32,      // start block address of segment 0
    pub cp_blkaddr: u32,            // start block address of checkpoint
    pub sit_blkaddr: u32,           // start block address of SIT
    pub nat_blkaddr: u32,           // start block address of NAT
    pub ssa_blkaddr: u32,           // start block address of SSA
    pub main_blkaddr: u32,          // start block address of main area
    pub root_ino: u32,              // root inode number
    pub node_ino: u32,              // node inode number
    pub meta_ino: u32,              // meta inode number
    pub uuid: [u8; 16],             // 128-bit uuid for volume
    pub volume_name: [u16; 512],    // volume name
    pub extension_count: u32,       // # of extensions
    pub extension_list: [[u8; 8]; 64],
    pub cp_payload: u32, // # of checkpoint trailing blocks for SIT bitmap

    // The following fields are not in the Fuchsia fork.
    pub kernel_version: [u8; 256],
    pub init_kernel_version: [u8; 256],
    pub feature: u32,
    pub encryption_level: u8,
    pub encryption_salt: [u8; 16],
    pub devices: [Device; 8],
    pub quota_file_ino: [u32; 3],
    pub hot_extension_count: u8,
    pub charset_encoding: u16,
    pub charset_encoding_flags: u16,
    pub stop_checkpoint_reason: [u8; 32],
    pub errors: [u8; 16],
    _reserved: [u8; 258],
    pub crc: u32,
}

pub const FEATURE_ENCRYPT: u32 = 0x00000001;
pub const FEATURE_EXTRA_ATTR: u32 = 0x00000008;
pub const FEATURE_PROJECT_QUOTA: u32 = 0x00000010;
pub const FEATURE_QUOTA_INO: u32 = 0x00000080;
pub const FEATURE_VERITY: u32 = 0x00000400;
pub const FEATURE_SB_CHKSUM: u32 = 0x00000800;
pub const FEATURE_CASEFOLD: u32 = 0x00001000;

pub const SUPPORTED_FEATURES: u32 = FEATURE_ENCRYPT
    | FEATURE_EXTRA_ATTR
    | FEATURE_PROJECT_QUOTA
    | FEATURE_QUOTA_INO
    | FEATURE_VERITY
    | FEATURE_SB_CHKSUM
    | FEATURE_CASEFOLD;

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub struct Device {
    pub path: [u8; 64],
    pub total_segments: u32,
}

impl SuperBlock {
    /// Reads the superblock from an device/image.
    pub async fn read_from_device(
        device: &dyn storage_device::Device,
        offset: u64,
    ) -> Result<Self, Error> {
        // Reads must be block aligned. Superblock is always first block of device.
        assert!(offset < BLOCK_SIZE as u64);
        let mut block = device.allocate_buffer(BLOCK_SIZE).await;
        device.read(0, block.as_mut()).await?;
        let data = block.to_vec();
        let buffer = &data[offset as usize..];
        let superblock =
            Self::read_from_bytes(buffer).map_err(|e| anyhow!("Failed to read superblock {e}"))?;
        ensure!(superblock.magic == F2FS_MAGIC, "Invalid F2fs magic number");

        // We only support 4kB block size so we can make some simplifying assumptions.
        ensure!(superblock.log_blocksize == 12, "Unsupported block size");
        // So many of the data structures assume 2MB segment size so just require that.
        ensure!(superblock.log_blocks_per_seg == 9, "Unsupported segment size");

        let feature = superblock.feature;
        ensure!(feature & !SUPPORTED_FEATURES == 0, "Unsupported feature set {feature:08x}");
        if superblock.feature & FEATURE_ENCRYPT != 0 {
            // We don't support encryption_level > 0 or salts.
            ensure!(
                superblock.encryption_level == 0 && superblock.encryption_salt == [0u8; 16],
                "Unsupported encryption features"
            );
        }

        #[cfg(not(fuzz))]
        if superblock.feature & FEATURE_SB_CHKSUM != 0 {
            let offset = superblock.checksum_offset as usize;
            ensure!(offset <= std::mem::size_of::<SuperBlock>(), "Invalid checksum_offset");
            let actual_checksum = f2fs_crc32(F2FS_MAGIC, &superblock.as_bytes()[..offset]);
            ensure!(superblock.crc == actual_checksum, "Bad superblock checksum");
        }
        if superblock.feature & FEATURE_CASEFOLD != 0 {
            // 1 here means 'UTF8 12.1.0' which is the version we support in Fxfs.
            ensure!(superblock.charset_encoding == 1, "Unsupported unicode charset");
            // We expect NO_COMPAT_FALLBACK to always be set.
            // Without this flag, missing hashes will be handled by exhaustive search of directories.
            const NO_COMPAT_FALLBACK: u16 = 2;
            let charset_encoding_flags = superblock.charset_encoding_flags;
            if charset_encoding_flags != NO_COMPAT_FALLBACK {
                log::warn!("Unsupported charset_encoding_flags {charset_encoding_flags:04x}");
            }
        }

        let cp_payload = superblock.cp_payload as usize;
        let sit_ver_bitmap_bytesize = (((superblock.segment_count_sit as u64 / 2)
            << superblock.log_blocks_per_seg)
            / 8) as usize;
        let nat_ver_bitmap_bytesize = (((superblock.segment_count_nat as u64 / 2)
            << superblock.log_blocks_per_seg)
            / 8) as usize;
        if cp_payload == 0 {
            ensure!(
                sit_ver_bitmap_bytesize + nat_ver_bitmap_bytesize <= MAX_BITMAP_BYTES,
                "SIT and NAT bitmaps exceed checkpoint capacity"
            );
        } else {
            ensure!(
                sit_ver_bitmap_bytesize <= cp_payload * BLOCK_SIZE
                    && nat_ver_bitmap_bytesize <= MAX_BITMAP_BYTES,
                "SIT or NAT bitmap exceeds capacity"
            );
        }

        Ok(superblock)
    }

    /// Gets the volume name as a string.
    pub fn get_volume_name(&self) -> Result<String, Error> {
        let volume_name = self.volume_name;
        let end = volume_name.iter().position(|&x| x == 0).unwrap_or(volume_name.len());
        String::from_utf16(&volume_name[..end]).map_err(|_| anyhow!("Bad UTF16 in volume name"))
    }

    /// Gets the total size of the filesystem in bytes.
    pub fn get_total_size(&self) -> u64 {
        (self.block_count as u64) * BLOCK_SIZE as u64
    }
}
