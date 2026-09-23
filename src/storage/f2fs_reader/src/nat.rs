// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::superblock::{BLOCK_SIZE, BLOCKS_PER_SEGMENT, SEGMENT_SIZE};
use anyhow::{Error, ensure};
use std::collections::HashMap;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// A mapping from logical node ID to block address.
#[repr(C, packed)]
#[derive(Copy, Clone, Default, Debug, Immutable, Unaligned, KnownLayout, FromBytes, IntoBytes)]
pub struct RawNatEntry {
    pub version: u8,
    pub ino: u32,
    pub block_addr: u32,
}

/// The number of NAT entries that fit in a single (4kb) block.
pub const NAT_ENTRY_PER_BLOCK: usize = BLOCK_SIZE / std::mem::size_of::<RawNatEntry>(); // 455

/// The "Node Address Table" provides a mapping from logical node ID to physical block address.
/// It boils down to an A/B segment and a bitmap (from the checkpoint) to select which to use.
/// The `nat_journal` is an additional small mapping of NAT updates that haven't yet been flushed.
pub struct Nat {
    /// Byte offset of NAT from start of device.
    device_offset: usize,
    /// There are two possible NAT locations for a given entry.
    /// These are in segment 2*n and 2*n+1.
    /// The bitmap tells us which one is valid.
    nat_bitmap: Vec<u8>,
    /// Copied from the summary block of the 'HotData' segment.
    /// This contains recent changes that trump what is on disk.
    pub nat_journal: HashMap<u32, RawNatEntry>,
}

impl Nat {
    pub fn new(
        nat_blkaddr: u32,
        nat_bitmap: Vec<u8>,
        nat_journal: HashMap<u32, RawNatEntry>,
    ) -> Self {
        let device_offset = nat_blkaddr as usize * BLOCK_SIZE;
        Self { device_offset, nat_bitmap, nat_journal }
    }

    /// Returns the offset of the block containing 'nid'
    pub fn get_nat_block_for_entry(&self, nid: u32) -> Result<u32, Error> {
        // 9 bytes per entry so 455 entries per 4kB block.
        let nat_block_ix = nid as usize / NAT_ENTRY_PER_BLOCK;
        // Alternating pairs of segments are use to hold NAT entries based on nat_bitmap.
        let segment_offset = nat_block_ix / BLOCKS_PER_SEGMENT * SEGMENT_SIZE * 2;
        // If bitmap bit is true, we read from odd segment, otherwise even.
        ensure!(
            nat_block_ix / 8 < self.nat_bitmap.len(),
            "Got request for nid {nid} which is beyond the bitmap size of {}",
            self.nat_bitmap.len()
        );
        let bitmap_offset =
            if (self.nat_bitmap[nat_block_ix / 8] << (nat_block_ix % 8)) & 0x80 == 0x80 {
                SEGMENT_SIZE
            } else {
                0
            };
        let byte_offset = self.device_offset
            + segment_offset
            + (nat_block_ix % BLOCKS_PER_SEGMENT) * BLOCK_SIZE
            + bitmap_offset;
        Ok((byte_offset / BLOCK_SIZE) as u32)
    }

    /// Returns the offset within the block containing 'nid'.
    pub fn get_nat_block_offset_for_entry(&self, nid: u32) -> usize {
        (nid as usize % NAT_ENTRY_PER_BLOCK) * std::mem::size_of::<RawNatEntry>()
    }
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct Summary {
    pub nid: u32,
    pub version: u8,
    pub ofs_in_node: u16,
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct SummaryFooter {
    pub entry_type: u8,
    pub check_sum: u32,
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct NatJournalEntry {
    pub ino: u32,
    pub entry: RawNatEntry,
}

const SUM_ENTRY_SIZE: usize = std::mem::size_of::<Summary>() * 512;
const N_NATS_SIZE: usize = 2;
const NAT_JOURNAL_SIZE: usize =
    BLOCK_SIZE - SUM_ENTRY_SIZE - N_NATS_SIZE - std::mem::size_of::<SummaryFooter>();
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct NatJournal {
    pub entries: [NatJournalEntry; NAT_JOURNAL_SIZE / std::mem::size_of::<NatJournalEntry>()],
    _reserved: [u8; NAT_JOURNAL_SIZE % std::mem::size_of::<NatJournalEntry>()],
}

/// SummaryBlock contains additional NAT entries to apply.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct SummaryBlock {
    pub entries: [Summary; 512],
    pub n_nats: u16,
    pub nat_journal: NatJournal,
    pub footer: SummaryFooter,
}
