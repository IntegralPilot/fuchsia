// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::crypto;
use crate::dir::InlineDentry;
use crate::reader::{NEW_ADDR, NULL_ADDR, Reader};
use crate::superblock::BLOCK_SIZE;
use crate::xattr::{XattrEntry, decode_xattr};
use anyhow::{Error, anyhow, ensure};
use bitflags::bitflags;
use std::collections::HashMap;
use std::fmt::Debug;
use storage_device::buffer::Buffer;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, Unaligned};

const NAME_MAX: usize = 255;
// The number of addresses that fit in an Inode block with a header and footer.
const INODE_BLOCK_MAX_ADDR: usize = 923;
// Hard coded constant from layout.h -- Number of 32-bit addresses that fit in an address block.
const ADDR_BLOCK_NUM_ADDR: u32 = 1018;

/// F2fs supports an extent tree and cached the largest extent for the file here.
/// (We don't make use of this.)
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct Extent {
    file_offset: u32,
    block_address: u32,
    len: u32,
}

#[derive(Copy, Clone, Debug, Immutable, FromBytes, IntoBytes)]
pub struct Mode(u16);
bitflags! {
    impl Mode: u16 {
        const RegularFile = 0o100000;
        const Directory = 0o040000;
    }
}

#[derive(Copy, Clone, Debug, Immutable, FromBytes, IntoBytes)]
pub struct AdviseFlags(u8);
bitflags! {
    impl AdviseFlags: u8 {
        const Encrypted = 0x04;
        const EncryptedName = 0x08;
        const Verity = 0x40;
    }
}

#[derive(Copy, Clone, Debug, Immutable, FromBytes, IntoBytes)]
pub struct InlineFlags(u8);
bitflags! {
    impl InlineFlags: u8 {
        const Xattr = 0b00000001;
        const Data = 0b00000010;
        const Dentry = 0b00000100;
        const ExtraAttr = 0b00100000;
    }
}

#[derive(Copy, Clone, Debug, Immutable, FromBytes, IntoBytes)]
pub struct Flags(u32);
bitflags! {
    impl Flags: u32 {
        const Casefold = 0x40000000;
    }
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct InodeHeader {
    pub mode: Mode,
    pub advise_flags: AdviseFlags,
    pub inline_flags: InlineFlags,
    pub uid: u32,
    pub gid: u32,
    pub links: u32,
    pub size: u64,
    pub block_size: u64,
    pub atime: i64,
    pub ctime: i64,
    pub mtime: i64,
    pub atime_nanos: u32,
    pub ctime_nanos: u32,
    pub mtime_nanos: u32,
    pub generation: u32,
    pub dir_depth: u32,
    pub xattr_nid: u32,
    pub flags: Flags,
    pub parent_inode: u32,
    pub name_len: u32,
    pub name: [u8; NAME_MAX],
    pub dir_level: u8,

    ext: Extent, // Holds the largest extent of this file, if using read extents. We ignore this.
}

/// This is optionally written after the header and before 'addr[0]' in Inode.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct InodeExtraAttr {
    pub extra_size: u16,
    pub inline_xattr_size: u16,
    pub project_id: u32,
    pub inode_checksum: u32,
    pub creation_time: u64,
    pub creation_time_nanos: u32,
    pub compressed_blocks: u64,
    pub compression_algorithm: u8,
    pub log_cluster_size: u8,
    pub compression_flags: u16,
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct InodeFooter {
    pub nid: u32,
    pub ino: u32,
    pub flag: u32,
    pub cp_ver: u64,
    pub next_blkaddr: u32,
}

/// Inode represents a file or directory and consumes one 4kB block in the metadata region.
///
/// An Inode's basic layout is as follows:
///    +--------------+
///    | InodeHeader  |
///    +--------------+
///    | i_addrs[923] |
///    +--------------+
///    | nids[5]      |
///    +--------------+
///    | InodeFooter  |
///    +--------------+
///
/// The i_addrs region consists of 32-bit block addresses to data associated with the inode.
/// Some or all of this may be repurposed for optional structures based on header flags:
///
///   * extra: Contains additional metadata. Consumes the first 9 entries of i_addrs.
///   * xattr: Extended attributes. Consumes the last 50 entries of i_addrs.
///   * inline_data: Consumes all remaining i_addrs. If used, no external data blocks are used.
///   * inline_dentry: Consumes all remaining i_addrs. If used, no external data blocks are used.
///
/// For inodes that do not contain inline data or inline dentry, the remaining i_addrs[] list
/// the block offsets for data blocks that contain the contents of the inode. A value of NULL_ADDR
/// indicates a zero page. A value of NEW_ADDR indicates a page that has not yet been allocated and
/// should be treated the same as a zero page for our purposes.
///
/// If a file contains more data than available i_addrs[], nids[] will be used.
///
/// nids[0] and nids[1] are what F2fs called "direct node" blocks. These contain nids (i.e. NAT
/// translated block addresses) to RawAddrBlock. Each RawAddrBlock contains up to 1018 block
/// offsets to data blocks.
///
/// If that is insufficient, nids[2] and nids[3] contain what F2fs calls "indirect node" blocks.
/// This is the same format as RawAddrBlock but each entry contains the nid of another
/// RawAddrBlock, providing another layer of indirection and thus the ability to reference
/// 1018^2 further blocks.
///
/// Finally, nids[4] may point at a "double indirect node" block. This adds one more layer of
/// indirection, allowing for a further 1018^3 blocks.
///
/// For sparse files, any individual blocks or pages of blocks (at any indirection level) may be
/// replaced with NULL_ADDR.
///
/// Block addressing starts at i_addrs and flows through each of nids[0..5] in order.
pub struct Inode {
    pub header: InodeHeader,
    pub extra: Option<InodeExtraAttr>,
    pub inline_data: Option<Box<[u8]>>,
    pub(super) inline_dentry: Option<InlineDentry>,
    pub(super) i_addrs: Vec<u32>,
    nids: [u32; 5],
    pub footer: InodeFooter,

    // These are loaded from additional nodes.
    nid_pages: HashMap<u32, Box<RawAddrBlock>>,
    pub xattr: Vec<XattrEntry>,

    // Crypto context, if present in xattr.
    pub context: Option<fscrypt::Context>,

    // Contains the set of block addresses in the data segment used by this inode.
    // This includes nids, indirect and double indirect address pages, and the xattr page
    pub block_addrs: Vec<u32>,
}

/// Both direct and indirect node address pages use this same format.
/// In the case of direct nodes, the addrs point to data blocks.
/// In the case of indirect and double-indirect nodes, the addrs point to nids of the next layer.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Immutable, KnownLayout, FromBytes, IntoBytes, Unaligned)]
pub struct RawAddrBlock {
    pub addrs: [u32; ADDR_BLOCK_NUM_ADDR as usize],
    _reserved:
        [u8; BLOCK_SIZE - std::mem::size_of::<InodeFooter>() - 4 * ADDR_BLOCK_NUM_ADDR as usize],
    pub footer: InodeFooter,
}

impl TryFrom<Buffer<'_>> for Box<RawAddrBlock> {
    type Error = Error;
    fn try_from(block: Buffer<'_>) -> Result<Self, Self::Error> {
        let raw = block
            .as_ptr_slice()
            .read::<RawAddrBlock>()
            .ok_or_else(|| anyhow!("Block size too small"))?;
        Ok(Box::new(raw))
    }
}

impl Debug for Inode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("Inode");
        out.field("header", &self.header);
        if let Some(extra) = &self.extra {
            out.field("extra", &extra);
        }
        if let Some(inline_dentry) = &self.inline_dentry {
            out.field("inline_dentry", &inline_dentry);
        }
        out.field("i_addrs", &self.i_addrs).field("footer", &self.footer);
        out.field("xattr", &self.xattr);
        out.finish()
    }
}

impl Inode {
    /// Attempt to load (and validate) an inode at a given nid.
    pub(super) async fn try_load(f2fs: &impl Reader, ino: u32) -> Result<Box<Inode>, Error> {
        let mut block_addrs = vec![];
        let mut raw_xattr = vec![];
        let mut this = {
            let block = f2fs.read_node(ino).await?;
            block_addrs.push(f2fs.get_nat_entry(ino).await?.block_addr);
            let data = block.to_vec();
            // Layout:
            //   header: InodeHeader
            //   extra: InodeExtraAttr # optional, based on header flag.
            //   i_addr: [u32; N]       # N = <=923 addr data or repurposed for inline fields.
            //   [u8; 200]      # optional, inline_xattr.
            //   [u32; 5]       # nids (for large block maps)
            //   InodeFooter
            let (header, rest): (Ref<_, InodeHeader>, _) = Ref::from_prefix(&data[..]).unwrap();
            let (rest, footer): (_, Ref<_, InodeFooter>) = Ref::from_suffix(rest).unwrap();
            ensure!(footer.ino == ino, "Footer ino doesn't match.");

            // nids are additional nodes pointing to data blocks. index has a specific meaning:
            //  - 0..2 => nid of nodes that contain addresses to data blocks.
            //  - 2..4 => nid of nodes that contain addresses to addresses of data blocks.
            //  - 5 => nid of a node that contains double-indirect addresses ot data blocks.
            let mut nids = [0u32; 5];
            nids.as_mut_bytes()
                .copy_from_slice(&rest[INODE_BLOCK_MAX_ADDR * 4..(INODE_BLOCK_MAX_ADDR + 5) * 4]);
            let rest = &rest[..INODE_BLOCK_MAX_ADDR * 4];

            let (extra, rest) = if header.inline_flags.contains(InlineFlags::ExtraAttr) {
                let (extra, _): (Ref<_, InodeExtraAttr>, _) = Ref::from_prefix(rest).unwrap();
                let extra_size = extra.extra_size as usize;
                ensure!(extra_size <= rest.len(), "Bad extra_size in inode");
                (Some((*extra).clone()), &rest[extra_size..])
            } else {
                (None, rest)
            };
            let rest = if header.inline_flags.contains(InlineFlags::Xattr) {
                // xattr always take up the last 50 i_addr slots. i.e. 200 bytes.
                ensure!(
                    rest.len() >= 200,
                    "Insufficient space for inline xattr. Likely bad extra_size."
                );
                raw_xattr.extend_from_slice(&rest[rest.len() - 200..]);
                &rest[..rest.len() - 200]
            } else {
                rest
            };

            let mut inline_data = None;
            let mut inline_dentry = None;
            let mut i_addrs: Vec<u32> = Vec::new();

            if header.inline_flags.contains(InlineFlags::Data) {
                // Inline data skips the first address slot then repurposes the remainder as data.
                ensure!(rest.len() >= 4, "Invalid inline data (insufficient remaining space)");
                let data = &rest[4..];
                ensure!(header.size <= data.len() as u64, "Invalid or corrupt inode.");
                inline_data = Some(data[..header.size as usize].to_vec().into_boxed_slice());
            } else if header.inline_flags.contains(InlineFlags::Dentry) {
                // Repurposes i_addr to store a set of directory entry records.
                inline_dentry = Some(InlineDentry::try_from_bytes(rest)?);
            } else {
                // &rest[..] is not necessarily 4-byte aligned so can't simply cast to [u32].
                i_addrs.resize(rest.len() / 4, 0);
                i_addrs.as_mut_bytes().copy_from_slice(&rest[..rest.len() / 4 * 4]);
            };

            Box::new(Self {
                header: (*header).clone(),
                extra,
                inline_data: inline_data.map(|x| x.into()),
                inline_dentry,
                i_addrs,
                nids,
                footer: (*footer).clone(),

                nid_pages: HashMap::new(),
                xattr: vec![],
                context: None,

                block_addrs,
            })
        };

        // Note that this call is done outside the above block to reduce the size of the future
        // that '.await' produces by ensuring any unnecessary local variables are out of scope.
        if this.header.xattr_nid != 0 {
            let node_block = f2fs.read_node(this.header.xattr_nid).await?;
            raw_xattr.extend_from_slice(&node_block.to_vec());
            this.block_addrs.push(f2fs.get_nat_entry(this.header.xattr_nid).await?.block_addr);
        }
        this.xattr = decode_xattr(&raw_xattr)?;

        this.context = crypto::try_read_context_from_xattr(&this.xattr)?;

        // The set of blocks making up the file begin with i_addrs. If more blocks are required
        // nids[0..5] are used. Zero pages (nid == NULL_ADDR) can be omitted at any level.
        for (i, nid) in this.nids.into_iter().enumerate() {
            if nid == NULL_ADDR {
                continue;
            }
            match i {
                0..2 => {
                    this.nid_pages.insert(nid, f2fs.read_node(nid).await?.try_into()?);
                    this.block_addrs.push(f2fs.get_nat_entry(nid).await?.block_addr);
                }
                2..4 => {
                    let indirect = Box::<RawAddrBlock>::try_from(f2fs.read_node(nid).await?)?;
                    this.block_addrs.push(f2fs.get_nat_entry(nid).await?.block_addr);
                    for nid in indirect.addrs {
                        if nid != NULL_ADDR {
                            this.nid_pages.insert(nid, f2fs.read_node(nid).await?.try_into()?);
                            this.block_addrs.push(f2fs.get_nat_entry(nid).await?.block_addr);
                        }
                    }
                    this.nid_pages.insert(nid, indirect);
                }
                4 => {
                    let double_indirect =
                        Box::<RawAddrBlock>::try_from(f2fs.read_node(nid).await?)?;
                    this.block_addrs.push(f2fs.get_nat_entry(nid).await?.block_addr);
                    for nid in double_indirect.addrs {
                        if nid != NULL_ADDR {
                            let indirect =
                                Box::<RawAddrBlock>::try_from(f2fs.read_node(nid).await?)?;
                            this.block_addrs.push(f2fs.get_nat_entry(nid).await?.block_addr);
                            for nid in indirect.addrs {
                                if nid != NULL_ADDR {
                                    this.nid_pages
                                        .insert(nid, f2fs.read_node(nid).await?.try_into()?);
                                    this.block_addrs
                                        .push(f2fs.get_nat_entry(nid).await?.block_addr);
                                }
                            }
                            this.nid_pages.insert(nid, indirect);
                        }
                    }
                    this.nid_pages.insert(nid, double_indirect);
                }
                _ => unreachable!(),
            }
        }

        Ok(this)
    }

    /// Walks through the data blocks of the file in order, handling sparse regions.
    /// Emits extents of (logical_block_num, physical_block_num, length).
    pub fn data_blocks(&self) -> DataBlocksIter<'_> {
        DataBlocksIter {
            iter: BlockIter { inode: self, stage: 0, offset: 0, a: 0, b: 0, c: 0 },
            next_block: None,
        }
    }

    /// Get the address of a specific logical data block.
    /// NULL_ADDR and NEW_ADDR should be considered sparse (unallocated) zero blocks.
    pub fn data_block_addr(&self, mut block_num: u32) -> u32 {
        let offset = block_num;

        if block_num < self.i_addrs.len() as u32 {
            return self.i_addrs[block_num as usize];
        }
        block_num -= self.i_addrs.len() as u32;

        // After we adjust for i_addrs, all offsets are simple constants.
        const NID0_END: u32 = ADDR_BLOCK_NUM_ADDR;
        const NID1_END: u32 = NID0_END + ADDR_BLOCK_NUM_ADDR;
        const NID2_END: u32 = NID1_END + ADDR_BLOCK_NUM_ADDR * ADDR_BLOCK_NUM_ADDR;
        const NID3_END: u32 = NID2_END + ADDR_BLOCK_NUM_ADDR * ADDR_BLOCK_NUM_ADDR;

        let mut iter = match block_num {
            ..NID0_END => {
                let a = block_num;
                BlockIter { inode: self, stage: 1, offset, a, b: 0, c: 0 }
            }
            ..NID1_END => {
                let a = block_num - NID0_END;
                BlockIter { inode: self, stage: 2, offset, a, b: 0, c: 0 }
            }
            ..NID2_END => {
                block_num -= NID1_END;
                let a = block_num / ADDR_BLOCK_NUM_ADDR;
                let b = block_num % ADDR_BLOCK_NUM_ADDR;
                BlockIter { inode: self, stage: 3, offset, a, b, c: 0 }
            }
            ..NID3_END => {
                block_num -= NID2_END;
                let a = block_num / ADDR_BLOCK_NUM_ADDR;
                let b = block_num % ADDR_BLOCK_NUM_ADDR;
                BlockIter { inode: self, stage: 4, offset, a, b, c: 0 }
            }
            _ => {
                block_num -= NID3_END;
                let a = block_num / ADDR_BLOCK_NUM_ADDR / ADDR_BLOCK_NUM_ADDR;
                let b = (block_num / ADDR_BLOCK_NUM_ADDR) % ADDR_BLOCK_NUM_ADDR;
                let c = block_num % ADDR_BLOCK_NUM_ADDR;
                BlockIter { inode: self, stage: 5, offset, a, b, c }
            }
        };
        if let Some((logical, physical)) = iter.next() {
            if logical == offset { physical } else { NULL_ADDR }
        } else {
            NULL_ADDR
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DataBlockExtent {
    /// The starting logical block number.
    pub logical_block_num: u32,
    /// The starting physical block number.
    pub physical_block_num: u32,
    /// The number of contiguous blocks in this extent.
    pub length: u32,
}

/// Iterates extents in the file. Will always create an extent break at end of file.
pub struct DataBlocksIter<'a> {
    iter: BlockIter<'a>,
    next_block: Option<(u32, u32)>,
}

impl Iterator for DataBlocksIter<'_> {
    type Item = DataBlockExtent;
    fn next(&mut self) -> Option<Self::Item> {
        let (log_start, phys_start) = self.next_block.take().or_else(|| self.iter.next())?;
        let mut len = 1;

        // Maximum file size is 2^32 blocks.
        // Iterators don't return errors so we ignore over-sized files to avoid overflow issues.
        if self.iter.inode.header.size > BLOCK_SIZE as u64 * u32::MAX as u64 {
            return None;
        }
        let file_end = (self.iter.inode.header.size.next_multiple_of(BLOCK_SIZE as u64)
            / BLOCK_SIZE as u64) as u32;

        loop {
            match self.iter.next() {
                Some((log, phys))
                    if Some(log) == log_start.checked_add(len)
                        && Some(phys) == phys_start.checked_add(len)
                        && log != file_end =>
                {
                    len += 1;
                }
                other => {
                    self.next_block = other;
                    return Some(DataBlockExtent {
                        logical_block_num: log_start,
                        physical_block_num: phys_start,
                        length: len,
                    });
                }
            }
        }
    }
}

struct BlockIter<'a> {
    inode: &'a Inode,
    stage: u32, // 0 -> i_addr, 1-> nids[0], 2 -> nids[1] -> ...
    offset: u32,
    a: u32, // depends on stage
    b: u32, // used for nids 2+ for indirection
    c: u32, // used for nids[4] for double-indirection.
}

impl Iterator for BlockIter<'_> {
    type Item = (u32, u32);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.stage {
                0 => {
                    // i_addrs
                    while let Some(&addr) = self.inode.i_addrs.get(self.a as usize) {
                        self.a += 1;
                        self.offset += 1;
                        if addr != NULL_ADDR && addr != NEW_ADDR {
                            return Some((self.offset - 1, addr));
                        }
                    }
                    self.stage += 1;
                    self.a = 0;
                }
                1..3 => {
                    // "direct"
                    let nid = self.inode.nids[self.stage as usize - 1];

                    if nid == NULL_ADDR || nid == NEW_ADDR {
                        self.stage += 1;
                        self.offset += ADDR_BLOCK_NUM_ADDR;
                    } else {
                        let addrs = self.inode.nid_pages.get(&nid).unwrap().addrs;
                        while let Some(&addr) = addrs.get(self.a as usize) {
                            self.a += 1;
                            self.offset += 1;
                            if addr != NULL_ADDR && addr != NEW_ADDR {
                                return Some((self.offset - 1, addr));
                            }
                        }
                        self.stage += 1;
                        self.a = 0;
                    }
                }

                3..5 => {
                    let nid = self.inode.nids[self.stage as usize - 1];
                    // "indirect"
                    if nid == NULL_ADDR || nid == NEW_ADDR {
                        self.stage += 1;
                        self.offset += ADDR_BLOCK_NUM_ADDR * ADDR_BLOCK_NUM_ADDR;
                    } else {
                        let addrs = self.inode.nid_pages.get(&nid).unwrap().addrs;
                        while let Some(&nid) = addrs.get(self.a as usize) {
                            if nid == NULL_ADDR || nid == NEW_ADDR {
                                self.a += 1;
                                self.offset += ADDR_BLOCK_NUM_ADDR;
                            } else {
                                let addrs = self.inode.nid_pages.get(&nid).unwrap().addrs;
                                while let Some(&addr) = addrs.get(self.b as usize) {
                                    self.b += 1;
                                    self.offset += 1;
                                    if addr != NULL_ADDR && addr != NEW_ADDR {
                                        return Some((self.offset - 1, addr));
                                    }
                                }
                                self.a += 1;
                                self.b = 0;
                            }
                        }
                        self.stage += 1;
                        self.a = 0;
                    }
                }

                5 => {
                    let nid = self.inode.nids[4];
                    // "double-indirect"
                    if nid != NULL_ADDR && nid != NEW_ADDR {
                        let addrs = self.inode.nid_pages.get(&nid).unwrap().addrs;
                        while let Some(&nid) = addrs.get(self.a as usize) {
                            if nid == NULL_ADDR || nid == NEW_ADDR {
                                self.a += 1;
                                self.offset += ADDR_BLOCK_NUM_ADDR * ADDR_BLOCK_NUM_ADDR;
                            } else {
                                let addrs = self.inode.nid_pages.get(&nid).unwrap().addrs;
                                while let Some(&nid) = addrs.get(self.b as usize) {
                                    if nid == NULL_ADDR || nid == NEW_ADDR {
                                        self.b += 1;
                                        self.offset += ADDR_BLOCK_NUM_ADDR;
                                    } else {
                                        let addrs = self.inode.nid_pages.get(&nid).unwrap().addrs;
                                        while let Some(&addr) = addrs.get(self.c as usize) {
                                            self.c += 1;
                                            self.offset += 1;
                                            if addr != NULL_ADDR && addr != NEW_ADDR {
                                                return Some((self.offset - 1, addr));
                                            }
                                        }
                                        self.b += 1;
                                        self.c = 0;
                                    }
                                }

                                self.a += 1;
                                self.b = 0;
                            }
                        }
                    }
                    self.stage += 1;
                }
                _ => {
                    break;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::nat::RawNatEntry;
    use crate::reader;
    use anyhow;
    use async_trait::async_trait;
    use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
    use zerocopy::FromZeros;

    /// A simple reader that can be filled explicitly with blocks to exercise inode.
    struct FakeReader {
        data: HashMap<u32, Box<[u8; 4096]>>,
        nids: HashMap<u32, Box<[u8; 4096]>>,
        allocator: BufferAllocator,
    }

    #[async_trait]
    impl reader::Reader for FakeReader {
        async fn read_raw_block(&self, block_addr: u32) -> Result<Buffer<'_>, Error> {
            match self.data.get(&block_addr) {
                None => Err(anyhow!("unexpected block {block_addr}")),
                Some(value) => {
                    let mut block = self.allocator.allocate_buffer(BLOCK_SIZE).await;
                    block.copy_from_slice(value.as_ref());
                    Ok(block)
                }
            }
        }

        async fn read_node(&self, nid: u32) -> Result<Buffer<'_>, Error> {
            match self.nids.get(&nid) {
                None => Err(anyhow!("unexpected nid {nid}")),
                Some(value) => {
                    let mut block = self.allocator.allocate_buffer(BLOCK_SIZE).await;
                    block.copy_from_slice(value.as_ref());
                    Ok(block)
                }
            }
        }

        fn fs_uuid(&self) -> &[u8; 16] {
            &[0; 16]
        }

        async fn get_nat_entry(&self, nid: u32) -> Result<RawNatEntry, Error> {
            Ok(RawNatEntry { ino: nid, block_addr: 0, ..Default::default() })
        }
    }

    // Builds a bare-bones inode block.
    fn build_inode(ino: u32) -> Box<[u8; BLOCK_SIZE]> {
        let mut header = InodeHeader::new_zeroed();
        let mut footer = InodeFooter::new_zeroed();
        let mut extra = InodeExtraAttr::new_zeroed();

        extra.extra_size = std::mem::size_of::<InodeExtraAttr>().try_into().unwrap();

        header.mode = Mode::RegularFile;
        header.inline_flags.set(InlineFlags::ExtraAttr, true);
        header.inline_flags.set(InlineFlags::Xattr, true);
        footer.ino = ino;

        let mut out = [0u8; BLOCK_SIZE];
        out[..std::mem::size_of::<InodeHeader>()].copy_from_slice(&header.as_bytes());
        out[std::mem::size_of::<InodeHeader>()
            ..std::mem::size_of::<InodeHeader>() + std::mem::size_of::<InodeExtraAttr>()]
            .copy_from_slice(&extra.as_bytes());
        out[BLOCK_SIZE - std::mem::size_of::<InodeFooter>()..].copy_from_slice(&footer.as_bytes());
        Box::new(out)
    }

    #[fuchsia::test]
    async fn test_xattr_bounds() {
        let mut reader = FakeReader {
            data: [].into(),
            nids: [(1, build_inode(1)), (2, [0u8; 4096].into()), (3, [0u8; 4096].into())].into(),
            allocator: BufferAllocator::new(BLOCK_SIZE, BufferSource::new(BLOCK_SIZE * 10)),
        };
        assert!(Inode::try_load(&reader, 1).await.is_ok());

        let header_len = std::mem::size_of::<InodeHeader>();
        let footer_len = std::mem::size_of::<InodeFooter>();
        let nids_len = std::mem::size_of::<u32>() * 5;
        let overheads = header_len + footer_len + nids_len;

        // Just enough room for xattrs.
        let mut extra = InodeExtraAttr::new_zeroed();
        extra.extra_size = (BLOCK_SIZE - overheads - 200) as u16;
        reader.nids.get_mut(&1).unwrap()[std::mem::size_of::<InodeHeader>()
            ..std::mem::size_of::<InodeHeader>() + std::mem::size_of::<InodeExtraAttr>()]
            .copy_from_slice(&extra.as_bytes());
        assert!(Inode::try_load(&reader, 1).await.is_ok());

        // No room for xattrs.
        let mut extra = InodeExtraAttr::new_zeroed();
        extra.extra_size = (BLOCK_SIZE - overheads - 199) as u16;
        reader.nids.get_mut(&1).unwrap()[std::mem::size_of::<InodeHeader>()
            ..std::mem::size_of::<InodeHeader>() + std::mem::size_of::<InodeExtraAttr>()]
            .copy_from_slice(&extra.as_bytes());
        assert!(Inode::try_load(&reader, 1).await.is_err());
    }
}

#[cfg(test)]
mod tests {
    use zerocopy::FromZeros;

    use super::*;

    fn last_addr_block(addr: u32) -> Box<RawAddrBlock> {
        let mut addr_block = RawAddrBlock::new_zeroed();
        addr_block.addrs[ADDR_BLOCK_NUM_ADDR as usize - 1] = addr;
        Box::new(addr_block)
    }

    #[test]
    fn test_data_iter() {
        // Fake up an inode with datablocks for the last block in each layer.
        //   1. The last i_addrs.
        //   2. The last nids[0] and nids[1].
        //   3. The last block of the last nids[2] and nids[3] blocks.
        //   4. The last block of the last block of nids[4] block.
        // All other blocks are unallocated.
        let header = InodeHeader::new_zeroed();
        let footer = InodeFooter::new_zeroed();
        let mut nids = [0u32; 5];
        let mut nid_pages = HashMap::new();
        nid_pages.insert(101, last_addr_block(1001));
        nid_pages.insert(102, last_addr_block(1002));

        let mut i_addrs: Vec<u32> = Vec::new();
        i_addrs.resize(INODE_BLOCK_MAX_ADDR, 0);
        i_addrs[0] = 100;
        i_addrs[1] = 101;
        i_addrs[2] = 102;
        i_addrs[INODE_BLOCK_MAX_ADDR - 1] = 1000;

        nids[0] = 101;
        nid_pages.insert(101, last_addr_block(1001));

        nids[1] = 102;
        nid_pages.insert(102, last_addr_block(1002));

        nids[2] = 103;
        nid_pages.insert(103, last_addr_block(104));
        nid_pages.insert(104, last_addr_block(1003));

        nids[3] = 105;
        nid_pages.insert(105, last_addr_block(106));
        nid_pages.insert(106, last_addr_block(1004));

        nids[4] = 107;
        nid_pages.insert(107, last_addr_block(108));
        nid_pages.insert(108, last_addr_block(109));
        nid_pages.insert(109, last_addr_block(1005));

        let inode = Box::new(Inode {
            header,
            extra: None,
            inline_data: None,
            inline_dentry: None,
            i_addrs,
            nids,
            footer: footer,

            nid_pages,
            xattr: vec![],
            context: None,

            block_addrs: vec![],
        });

        // Also test data_block_addr while we're walking.
        assert_eq!(inode.data_block_addr(0), 100);

        let mut iter = inode.data_blocks();
        assert_eq!(
            iter.next(),
            Some(DataBlockExtent { logical_block_num: 0, physical_block_num: 100, length: 3 })
        );

        let mut block_num = 922;
        assert_eq!(
            iter.next(),
            Some(DataBlockExtent {
                logical_block_num: block_num,
                physical_block_num: 1000,
                length: 1
            })
        ); // i_addrs
        assert_eq!(inode.data_block_addr(block_num), 1000);
        block_num += ADDR_BLOCK_NUM_ADDR;
        assert_eq!(
            iter.next(),
            Some(DataBlockExtent {
                logical_block_num: block_num,
                physical_block_num: 1001,
                length: 1
            })
        ); // nids[0]
        assert_eq!(inode.data_block_addr(block_num), 1001);
        block_num += ADDR_BLOCK_NUM_ADDR;
        assert_eq!(
            iter.next(),
            Some(DataBlockExtent {
                logical_block_num: block_num,
                physical_block_num: 1002,
                length: 1
            })
        ); // nids[1]
        assert_eq!(inode.data_block_addr(block_num), 1002);
        block_num += ADDR_BLOCK_NUM_ADDR * ADDR_BLOCK_NUM_ADDR;
        assert_eq!(
            iter.next(),
            Some(DataBlockExtent {
                logical_block_num: block_num,
                physical_block_num: 1003,
                length: 1
            })
        ); // nids[2]
        assert_eq!(inode.data_block_addr(block_num), 1003);
        block_num += ADDR_BLOCK_NUM_ADDR * ADDR_BLOCK_NUM_ADDR;
        assert_eq!(
            iter.next(),
            Some(DataBlockExtent {
                logical_block_num: block_num,
                physical_block_num: 1004,
                length: 1
            })
        ); // nids[3]
        assert_eq!(inode.data_block_addr(block_num), 1004);
        block_num += ADDR_BLOCK_NUM_ADDR * ADDR_BLOCK_NUM_ADDR * ADDR_BLOCK_NUM_ADDR;
        assert_eq!(
            iter.next(),
            Some(DataBlockExtent {
                logical_block_num: block_num,
                physical_block_num: 1005,
                length: 1
            })
        ); // nids[4]
        assert_eq!(inode.data_block_addr(block_num), 1005);
        assert_eq!(iter.next(), None);
        assert_eq!(inode.data_block_addr(block_num - 1), 0);
        assert_eq!(inode.data_block_addr(block_num + 1), 0);
    }
}
