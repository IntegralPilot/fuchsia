// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::superblock::{BLOCK_SIZE, BLOCKS_PER_SEGMENT, F2FS_MAGIC, SEGMENT_SIZE, f2fs_crc32};
use anyhow::{Error, anyhow, ensure};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

const MAX_ACTIVE_NODE_LOGS: usize = 8;
const MAX_ACTIVE_DATA_LOGS: usize = 8;
const MAX_ACTIVE_LOGS: usize = 16;
pub const CP_ORPHAN_PRESENT_FLAG: u32 = 0x2;
pub const CKPT_FLAG_COMPACT_SUMMARY: u32 = 0x4;
pub const ORPHANS_PER_BLOCK: usize = 1020;

#[derive(Clone, Copy, Debug, Eq, PartialEq, FromBytes, Immutable, IntoBytes, KnownLayout)]
#[repr(C, packed)]
pub struct OrphanBlock {
    pub ino: [u32; ORPHANS_PER_BLOCK],
    pub reserved: u32,
    pub blk_addr: u16,
    pub blk_count: u16,
    pub entry_count: u32,
    pub check_sum: u32,
}

#[derive(Debug, Eq, PartialEq, FromBytes, Immutable, IntoBytes, KnownLayout)]
#[repr(C, packed)]
pub struct CheckpointHeader {
    pub checkpoint_ver: u64,
    pub user_block_count: u64,
    pub valid_block_count: u64,
    pub rsvd_segment_count: u32,
    pub overprov_segment_count: u32,
    pub free_segment_count: u32,
    pub cur_node_segno: [u32; MAX_ACTIVE_NODE_LOGS],
    pub cur_node_blkoff: [u16; MAX_ACTIVE_NODE_LOGS],
    pub cur_data_segno: [u32; MAX_ACTIVE_DATA_LOGS],
    pub cur_data_blkoff: [u16; MAX_ACTIVE_DATA_LOGS],
    pub ckpt_flags: u32,
    pub cp_pack_total_block_count: u32,
    pub cp_pack_start_sum: u32,
    pub valid_node_count: u32,
    pub valid_inode_count: u32,
    pub next_free_nid: u32,
    pub sit_ver_bitmap_bytesize: u32,
    pub nat_ver_bitmap_bytesize: u32,
    pub checksum_offset: u32,
    pub elapsed_time: u64,
    pub alloc_type: [u8; MAX_ACTIVE_LOGS],
    // SIT bitmap follows.
    // NAT bitmap follows.
}

#[derive(Debug)]
pub struct CheckpointPack {
    pub header: CheckpointHeader,
    pub nat_bitmap: Vec<u8>,
}

pub const MAX_BITMAP_BYTES: usize =
    BLOCK_SIZE - std::mem::size_of::<u32>() - std::mem::size_of::<CheckpointHeader>(); // 3900

impl CheckpointPack {
    pub async fn read_from_device(
        device: &dyn storage_device::Device,
        offset: u64,
        cp_payload: u32,
    ) -> Result<Self, Error> {
        let mut segment = device.allocate_buffer(SEGMENT_SIZE).await;
        device.read(offset, segment.as_mut()).await?;
        let data = segment.to_vec();
        Self::parse_checkpoint(&data, cp_payload)
    }

    pub fn parse_checkpoint(segment: &[u8], cp_payload: u32) -> Result<Self, Error> {
        ensure!(
            (cp_payload as usize) <= BLOCKS_PER_SEGMENT - 3,
            "cp_payload exceeds segment capacity"
        );
        ensure!(
            segment.len() >= std::mem::size_of::<CheckpointHeader>(),
            "Segment too short for checkpoint"
        );
        let header =
            CheckpointHeader::read_from_bytes(&segment[..std::mem::size_of::<CheckpointHeader>()])
                .map_err(|_| anyhow!("Invalid checkpoint header"))?;
        ensure!(
            header.cp_pack_total_block_count > 0
                && (header.cp_pack_total_block_count as usize) <= BLOCKS_PER_SEGMENT,
            "Invalid cp_pack_total_block_count"
        );
        ensure!(
            segment.len() >= header.cp_pack_total_block_count as usize * BLOCK_SIZE,
            "Segment too short for checkpoint pack"
        );
        let len = header.checksum_offset as usize;
        ensure!(len == BLOCK_SIZE - std::mem::size_of::<u32>(), "Bad checkpoint offset");
        #[cfg(not(fuzz))]
        {
            let mut checksum: u32 = 0;
            checksum
                .as_mut_bytes()
                .copy_from_slice(&segment[len..len + std::mem::size_of::<u32>()]);
            let crc32 = f2fs_crc32(F2FS_MAGIC, &segment[..len]);
            ensure!(crc32 == checksum, "Bad Checkpoint checksum ({crc32:08x} != {checksum:08x})");
        }
        let sit_ver_bitmap_bytesize = header.sit_ver_bitmap_bytesize as usize;
        let nat_ver_bitmap_bytesize = header.nat_ver_bitmap_bytesize as usize;
        ensure!(sit_ver_bitmap_bytesize > 0, "Invalid sit_bitmap size");
        ensure!(nat_ver_bitmap_bytesize > 0, "Invalid nat_bitmap size");
        ensure!(
            nat_ver_bitmap_bytesize <= MAX_BITMAP_BYTES,
            "NAT bitmap exceeds checkpoint capacity"
        );

        let nat_bitmap = if cp_payload == 0 {
            ensure!(
                sit_ver_bitmap_bytesize + nat_ver_bitmap_bytesize <= MAX_BITMAP_BYTES,
                "SIT and NAT bitmaps exceed checkpoint capacity"
            );
            let nat_bitmap_start =
                std::mem::size_of::<CheckpointHeader>() + sit_ver_bitmap_bytesize;
            let nat_bitmap_end = nat_bitmap_start + nat_ver_bitmap_bytesize;
            segment[nat_bitmap_start..nat_bitmap_end].to_vec()
        } else {
            ensure!(
                sit_ver_bitmap_bytesize <= cp_payload as usize * BLOCK_SIZE,
                "SIT bitmap exceeds cp_payload capacity"
            );
            let nat_bitmap_start = std::mem::size_of::<CheckpointHeader>();
            let nat_bitmap_end = nat_bitmap_start + nat_ver_bitmap_bytesize;
            segment[nat_bitmap_start..nat_bitmap_end].to_vec()
        };

        ensure!(header.cp_pack_start_sum > cp_payload, "cp_pack_start_sum too small");
        ensure!(
            header.cp_pack_start_sum < header.cp_pack_total_block_count,
            "cp_pack_start_sum exceeds total blocks in checkpoint pack"
        );
        let backup_header_offset =
            (header.cp_pack_total_block_count as usize - 1) * BLOCK_SIZE as usize;
        let backup_header = CheckpointHeader::read_from_bytes(
            &segment[backup_header_offset
                ..backup_header_offset + std::mem::size_of::<CheckpointHeader>()],
        )
        .map_err(|_| anyhow!("Invalid backup header"))?;
        // If the backup copy is bad, fail this checkpoint (same as f2fs Fuchsia).
        ensure!(backup_header == header, "CheckpointHeader and backup differ");
        Ok(Self { header, nat_bitmap })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Some basic robustness coverage.
    #[test]
    fn test_checkpoint_parsing() {
        assert!(CheckpointPack::parse_checkpoint(&[], 0).is_err());

        let mut segment = Vec::new();
        segment.resize(SEGMENT_SIZE, 0);
        let mut header =
            CheckpointHeader::read_from_bytes(&segment[..std::mem::size_of::<CheckpointHeader>()])
                .unwrap();
        header.checksum_offset = (BLOCK_SIZE - 4) as u32;
        header.cp_pack_start_sum = 1;
        header.cp_pack_total_block_count = 100;

        // helper to copy header into segment and set the checksum to a valid value.
        let set_header = |segment: &mut [u8], header: &CheckpointHeader| {
            segment[..std::mem::size_of::<CheckpointHeader>()].copy_from_slice(header.as_bytes());
            let crc32 = f2fs_crc32(F2FS_MAGIC, &segment[..header.checksum_offset as usize]);
            segment[header.checksum_offset as usize..header.checksum_offset as usize + 4]
                .copy_from_slice(crc32.as_bytes());
        };

        // Bad checksum offset.
        {
            header.checksum_offset = SEGMENT_SIZE as u32 - 3;
            segment[..std::mem::size_of::<CheckpointHeader>()].copy_from_slice(header.as_bytes());
            assert!(CheckpointPack::parse_checkpoint(&segment, 0).is_err());
        }
        // Bad SIT size.
        {
            header.checksum_offset = (BLOCK_SIZE - 4) as u32;
            header.sit_ver_bitmap_bytesize = SEGMENT_SIZE as u32;
            set_header(&mut segment, &header);
            assert!(CheckpointPack::parse_checkpoint(&segment, 0).is_err());
        }
        // Bad NAT size.
        {
            header.sit_ver_bitmap_bytesize = 64;
            header.nat_ver_bitmap_bytesize = SEGMENT_SIZE as u32;
            set_header(&mut segment, &header);
            assert!(CheckpointPack::parse_checkpoint(&segment, 0).is_err());
        }
        // Bad SIT+NAT size when cp_payload == 0 (e.g. 2000 + 2000 > 3900).
        {
            header.sit_ver_bitmap_bytesize = 2000;
            header.nat_ver_bitmap_bytesize = 2000;
            set_header(&mut segment, &header);
            assert!(CheckpointPack::parse_checkpoint(&segment, 0).is_err());
        }
        // Bad cp_pack_total_block_count (more than one segment).
        {
            header.sit_ver_bitmap_bytesize = 64;
            header.nat_ver_bitmap_bytesize = 256;
            header.cp_pack_total_block_count = 2048;
            set_header(&mut segment, &header);
            assert!(CheckpointPack::parse_checkpoint(&segment, 0).is_err());
        }
        // Bad checksum offset in block 0 (e.g. pointing into block 1 or 2).
        {
            header.checksum_offset = 8192 - 4;
            header.cp_pack_total_block_count = 100;
            set_header(&mut segment, &header);
            assert!(CheckpointPack::parse_checkpoint(&segment, 0).is_err());
        }
        // Bad cp_pack_start_sum (<= cp_payload).
        {
            header.checksum_offset = (BLOCK_SIZE - 4) as u32;
            header.cp_pack_start_sum = 0;
            set_header(&mut segment, &header);
            assert!(CheckpointPack::parse_checkpoint(&segment, 0).is_err());
        }
        // Bad cp_pack_start_sum (>= cp_pack_total_block_count).
        {
            header.cp_pack_start_sum = 100;
            header.cp_pack_total_block_count = 100;
            set_header(&mut segment, &header);
            assert!(CheckpointPack::parse_checkpoint(&segment, 0).is_err());
        }
        // Success when cp_payload == 0.
        {
            header.checksum_offset = (BLOCK_SIZE - 4) as u32;
            header.sit_ver_bitmap_bytesize = 64;
            header.nat_ver_bitmap_bytesize = 256;
            header.cp_pack_start_sum = 1;
            header.cp_pack_total_block_count = 100;
            set_header(&mut segment, &header);
            segment.copy_within(..std::mem::size_of::<CheckpointHeader>(), BLOCK_SIZE * 99);
            let result = CheckpointPack::parse_checkpoint(&segment, 0);
            assert!(result.is_ok(), "{:?}", result);
        }
        // Bad cp_payload (> BLOCKS_PER_SEGMENT - 3).
        {
            assert!(CheckpointPack::parse_checkpoint(&segment, 1000).is_err());
        }
        // Success when cp_payload > 0 (NAT bitmap read from start of bitmap area).
        {
            header.sit_ver_bitmap_bytesize = 4096;
            header.nat_ver_bitmap_bytesize = 256;
            header.cp_pack_start_sum = 2;
            header.cp_pack_total_block_count = 100;
            // Write a test pattern to NAT bitmap location (offset 192).
            let nat_start = std::mem::size_of::<CheckpointHeader>();
            segment[nat_start..nat_start + 256].fill(0x5A);
            set_header(&mut segment, &header);
            segment.copy_within(..std::mem::size_of::<CheckpointHeader>(), BLOCK_SIZE * 99);
            let result = CheckpointPack::parse_checkpoint(&segment, 1);
            assert!(result.is_ok(), "{:?}", result);
            assert_eq!(result.unwrap().nat_bitmap, vec![0x5A; 256]);
        }
        // Truncated slice smaller than BLOCK_SIZE does not panic.
        {
            assert!(CheckpointPack::parse_checkpoint(&segment[..512], 0).is_err());
        }
        // Truncated slice smaller than cp_pack_total_block_count * BLOCK_SIZE.
        {
            assert!(CheckpointPack::parse_checkpoint(&segment[..BLOCK_SIZE * 50], 0).is_err());
        }
    }
}
