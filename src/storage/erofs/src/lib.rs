// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! EROFS filesystem.

use bitflags::bitflags;
use crc::{CRC_32_ISCSI, Crc};
use std::sync::Arc;
use thiserror::Error;
use zerocopy::IntoBytes;
use zerocopy::byteorder::little_endian::U32 as LEU32;

pub mod readers;
use readers::{Reader, ReaderError, ReaderExt};

pub mod format;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FeatureCompat: u32 {
        /// If this feature is set, the checksum field in the superblock is valid and should be
        /// used to verify the superblock integrity.
        const SB_CHKSUM = 0x00000001;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FeatureIncompat: u32 {
        /// If this feature is set, compressed data is right-aligned and the beginning is padded
        /// with zeros, which the decompression logic needs to trim to find the real data. This is
        /// done to support a memory optimization when decompressing in linux.
        const ZERO_PADDING = 0x00000001;
    }
}

bitflags! {
    /// Flags for various compression behaviors, stored per-inode in the compression header when
    /// the CompressedFull or CompressedCompact data layout are used.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CompressionAdvise: u16 {
        /// There are two possible entry table layouts when using the CompressedCompact data
        /// layout. This indicates that we should expect the even more compact one.
        const COMPACTED_2B = 0x0001;
    }
}

/// The bit width of the low value field in compact cluster index entries (clusterofs / delta0).
/// This is fixed at 12 bits for supported block sizes (512B to 4KB).
pub const COMPACT_ENTRY_LOBITS: u32 = 12;

/// Errors that can occur while interacting with an EROFS image.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum ErofsError {
    #[error("Unsupported feature incompat flags: 0x{:X}. Only 0x{:X} is supported", _0, _1)]
    UnsupportedFeatureIncompat(u32, u32),

    #[error("Parsing error: {}", _0)]
    Parse(#[from] ParsingError),
    #[error("Reader error: {}", _0)]
    ReadError(#[from] ReaderError),
}

#[cfg(target_os = "fuchsia")]
impl ErofsError {
    pub fn to_status(self) -> zx::Status {
        match self {
            Self::UnsupportedFeatureIncompat(_, _) => zx::Status::NOT_SUPPORTED,
            Self::Parse(_) => zx::Status::IO_DATA_INTEGRITY,
            Self::ReadError(_) => zx::Status::IO,
        }
    }
}

/// Errors that can occur during parsing of an EROFS image.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum ParsingError {
    #[error("Invalid super block magic: 0x{:X}, should be 0x{:X}", _0, format::EROFS_MAGIC)]
    InvalidSuperBlockMagic(u32),
    #[error("Checksum mismatch: expected 0x{:X}, computed 0x{:X}", _0, _1)]
    ChecksumMismatch(u32, u32),
    #[error("Invalid block size bits: {}, must be between 9 and 12", _0)]
    InvalidBlockSizeBits(u8),

    #[error("Invalid inode data layout: 0x{:X}", _0)]
    InvalidInodeDataLayout(u16),
    #[error("Expected compressed inode layout, found {:?}", _0)]
    UnexpectedInodeDataLayout(InodeDataLayout),
    #[error("Missing compression map header on compressed inode")]
    MissingCompressionHeader,
    #[error("Unexpected compression algorithm type: {}", _0)]
    UnexpectedCompressionAlgorithm(u8),
    #[error("Invalid directory entry")]
    InvalidDirectoryEntry,
    #[error("Invalid file type: {}", _0)]
    InvalidFileType(u8),
    #[error("Directory entry name was not valid utf8: {}", _0)]
    InvalidDirectoryEntryName(#[source] std::str::Utf8Error),
    #[error("Inline data layout missing inline data")]
    InlineDataLayoutMissingInlineData,

    #[error("Invalid root node")]
    InvalidRootNode,
    #[error("Node has an invalid U value for its data layout")]
    InvalidUValue,
    #[error("Invalid nid: {}", _0)]
    InvalidNid(u64),
    #[error("Integer overflow during calculation")]
    Overflow,
    #[error("Decompression failed: {}", _0)]
    DecompressionFailed(#[from] lz4::Error),
    #[error("Missing shared xattr area but inode has shared xattrs")]
    MissingSharedXattrArea,
    #[error("Xattr entry extends past the end of the inline xattr region")]
    XattrEntryOutOfBounds,
    #[error("Invalid xattr namespace index: {}", _0)]
    InvalidXattrNamespace(u8),

    #[error("Invalid logical cluster type {}", _0)]
    InvalidLClusterType(u16),
    #[error("Expected HEAD logical cluster at lcn {}", _0)]
    ExpectedHeadLCluster(u64),
    #[error(
        "Logical cluster number {} out of bounds (total clusters: {})",
        cluster_index,
        total_lclusters
    )]
    LClusterOutOfBounds { cluster_index: u64, total_lclusters: u64 },
    #[error("Invalid NonHead delta0 {} at cluster index {}", delta0, cluster_index)]
    InvalidLClusterDelta { cluster_index: u64, delta0: u16 },
    #[error("Corrupted compact cluster index pack: {}", _0)]
    CorruptedCompactClusterPack(&'static str),
    #[error(
        "Compact cluster pack offset {}..{} out of bounds (pack size: {})",
        byte_offset,
        byte_offset + 4,
        pack_size
    )]
    CompactPackOutOfBounds { byte_offset: usize, pack_size: usize },
    #[error("Logical offset {} is before start of initial cluster {}", offset, cluster_start)]
    InvalidClusterOffset { offset: u64, cluster_start: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InodeDataUnion {
    DataBlkAddrPlain(u32),
    DataBlkAddrInline(u32),
    CompressedBlocks(u32),
}

impl InodeDataUnion {
    fn parse(data: [u8; 4], format: InodeFormat) -> Self {
        match format.data_layout {
            InodeDataLayout::FlatPlain => {
                InodeDataUnion::DataBlkAddrPlain(u32::from_le_bytes(data))
            }
            // Technically this is only valid for inline data where the size is more than a block.
            InodeDataLayout::FlatInline => {
                InodeDataUnion::DataBlkAddrInline(u32::from_le_bytes(data))
            }
            InodeDataLayout::CompressedFull | InodeDataLayout::CompressedCompact => {
                InodeDataUnion::CompressedBlocks(u32::from_le_bytes(data))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeInner {
    inode_offset: u64,
    format: InodeFormat,
    mode: u16,
    size: u64,
    data_union: InodeDataUnion,
    ino: u32,
    nid: u64,
    link_count: u32,
    uid: u32,
    gid: u32,
    mtime_ns: u64,
    xattr_icount: u16,
    compression_header: Option<CompressionHeader>,
}

impl NodeInner {
    fn is_dir(&self) -> bool {
        (self.mode & 0xf000) == 0x4000
    }

    fn is_symlink(&self) -> bool {
        (self.mode & 0xf000) == 0xa000
    }

    fn inode_offset(&self) -> u64 {
        self.inode_offset
    }

    /// Interpret the u field as a block address. This is only a valid interpretation on FlatPlain,
    /// or on FlatInline if the size is larger than a block.
    fn blkaddr(&self, block_size: u64) -> Option<u64> {
        match self.data_union {
            InodeDataUnion::DataBlkAddrPlain(addr) => Some(addr.into()),
            InodeDataUnion::DataBlkAddrInline(addr) => {
                debug_assert!(self.size / block_size > 0);
                Some(addr.into())
            }
            InodeDataUnion::CompressedBlocks(_) => None,
        }
    }

    /// Safely calculate the on-disk offset for a read in this nodes data. This doesn't check out
    /// of bounds errors.
    fn blkaddr_offset(&self, block_size: u64, offset: u64) -> Result<u64, ParsingError> {
        self.blkaddr(block_size)
            .ok_or(ParsingError::InvalidUValue)?
            .checked_mul(block_size)
            .ok_or(ParsingError::Overflow)?
            .checked_add(offset)
            .ok_or(ParsingError::Overflow)
    }

    fn metadata_size(&self) -> u64 {
        match self.format.version {
            InodeVersion::Compact => 32,
            InodeVersion::Extended => 64,
        }
    }

    fn inline_xattr_size(&self) -> u64 {
        if self.xattr_icount == 0 { 0 } else { ((self.xattr_icount as u64 - 1) * 4) + 12 }
    }

    /// Offset immediately following this node's metadata and inline xattrs.
    fn metadata_end_offset(&self) -> Result<u64, ParsingError> {
        self.inode_offset()
            .checked_add(self.metadata_size())
            .ok_or(ParsingError::Overflow)?
            .checked_add(self.inline_xattr_size())
            .ok_or(ParsingError::Overflow)
    }

    /// Offset of the compression MapHeader (8-byte aligned after metadata and inline xattrs).
    fn map_header_offset(&self) -> Result<u64, ParsingError> {
        let metadata_end = self.metadata_end_offset()?;
        Ok(metadata_end.next_multiple_of(8))
    }

    /// Offset of the start of the logical cluster index table.
    fn index_table_offset(&self) -> Result<u64, ParsingError> {
        let map_header_offset = self.map_header_offset()?;
        match self.format.data_layout {
            InodeDataLayout::CompressedFull => {
                Ok(map_header_offset + format::LEGACY_MAP_HEADER_SIZE)
            }
            InodeDataLayout::CompressedCompact => {
                Ok(map_header_offset + std::mem::size_of::<format::CompressionMapHeader>() as u64)
            }
            _ => Err(ParsingError::UnexpectedInodeDataLayout(self.format.data_layout)),
        }
    }

    /// Returns the compression header for this node, if it has a compressed data layout.
    pub fn compression_header(&self) -> Option<&CompressionHeader> {
        self.compression_header.as_ref()
    }

    /// Returns the position and layout of a compact logical cluster index entry.
    pub fn compact_entry_pos(
        &self,
        block_size: u64,
        cluster_index: u64,
    ) -> Result<CompactEntry, ParsingError> {
        if self.format.data_layout != InodeDataLayout::CompressedCompact {
            return Err(ParsingError::UnexpectedInodeDataLayout(self.format.data_layout));
        }
        let total_clusters = self.total_lclusters(block_size);
        if cluster_index >= total_clusters {
            return Err(ParsingError::LClusterOutOfBounds {
                cluster_index,
                total_lclusters: total_clusters,
            });
        }

        let table_offset = self.index_table_offset()?;
        let header =
            self.compression_header.as_ref().ok_or(ParsingError::MissingCompressionHeader)?;
        let is_compact_2b = header.advise.contains(CompressionAdvise::COMPACTED_2B);

        // Number of 4B entries needed to align to a 32-byte boundary (for 2B packs)
        let initial_4b_count = ((32 - (table_offset % 32)) / 4) & 7;
        let middle_2b_count = if is_compact_2b && initial_4b_count < total_clusters {
            (total_clusters - initial_4b_count) & !15
        } else {
            0
        };

        let (pack_pos, layout, entry_index) = if cluster_index < initial_4b_count {
            let pack_idx = cluster_index / 2;
            (table_offset + pack_idx * 8, CompactPackLayout::Pack4B, (cluster_index % 2) as usize)
        } else if cluster_index < initial_4b_count + middle_2b_count {
            let rel_lcn = cluster_index - initial_4b_count;
            let base_2b_offset = table_offset + initial_4b_count * 4;
            let pack_idx = rel_lcn / 16;
            (base_2b_offset + pack_idx * 32, CompactPackLayout::Pack2B, (rel_lcn % 16) as usize)
        } else {
            let rel_lcn = cluster_index - initial_4b_count - middle_2b_count;
            let base_trailing_offset = table_offset + initial_4b_count * 4 + middle_2b_count * 2;
            let pack_idx = rel_lcn / 2;
            (base_trailing_offset + pack_idx * 8, CompactPackLayout::Pack4B, (rel_lcn % 2) as usize)
        };

        Ok(CompactEntry { pack_offset_bytes: pack_pos, layout, entry_index })
    }

    /// Offset immediately following the logical cluster index table. For files with inline data,
    /// this is where the inline data is stored.
    pub fn index_end_offset(&self, block_size: u64) -> Result<u64, ParsingError> {
        let max_lcn = self.total_lclusters(block_size);
        match self.format.data_layout {
            InodeDataLayout::CompressedFull => {
                let index_table_offset = self.index_table_offset()?;
                index_table_offset
                    .checked_add(max_lcn.checked_mul(8).ok_or(ParsingError::Overflow)?)
                    .ok_or(ParsingError::Overflow)
            }
            InodeDataLayout::CompressedCompact => {
                if max_lcn == 0 {
                    return self.index_table_offset();
                }
                let pos = self.compact_entry_pos(block_size, max_lcn - 1)?;
                pos.pack_offset_bytes.checked_add(pos.pack_size()).ok_or(ParsingError::Overflow)
            }
            _ => Err(ParsingError::UnexpectedInodeDataLayout(self.format.data_layout)),
        }
    }

    /// Returns the total number of logical clusters for this node.
    pub fn total_lclusters(&self, lcluster_size: u64) -> u64 {
        self.size.div_ceil(lcluster_size)
    }

    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn ino(&self) -> u32 {
        self.ino
    }
    pub fn nid(&self) -> u64 {
        self.nid
    }
    pub fn link_count(&self) -> u32 {
        self.link_count
    }
    pub fn uid(&self) -> u32 {
        self.uid
    }
    pub fn gid(&self) -> u32 {
        self.gid
    }
    pub fn mtime_ns(&self) -> u64 {
        self.mtime_ns
    }
    pub fn mode(&self) -> u16 {
        self.mode
    }

    /// Returns the storage size in bytes taken up by this node on disk.
    pub fn storage_size(&self, block_size: u64) -> u64 {
        match self.data_union {
            InodeDataUnion::CompressedBlocks(blocks) => (blocks as u64) * block_size,
            _ => self.size,
        }
    }
}

/// A directory node in the EROFS image.
#[derive(Debug, Clone)]
pub struct DirectoryNode(NodeInner);

impl std::ops::Deref for DirectoryNode {
    type Target = NodeInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// File type for a directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileType {
    #[default]
    Unknown = 0,
    RegFile = 1,
    Dir = 2,
    ChrDev = 3,
    BlkDev = 4,
    Fifo = 5,
    Sock = 6,
    Symlink = 7,
}

impl TryFrom<u8> for FileType {
    type Error = ParsingError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(FileType::Unknown),
            1 => Ok(FileType::RegFile),
            2 => Ok(FileType::Dir),
            3 => Ok(FileType::ChrDev),
            4 => Ok(FileType::BlkDev),
            5 => Ok(FileType::Fifo),
            6 => Ok(FileType::Sock),
            7 => Ok(FileType::Symlink),
            _ => Err(ParsingError::InvalidFileType(value)),
        }
    }
}

/// A directory entry in the EROFS image.
#[derive(Debug, Clone, Default)]
pub struct DirectoryEntry {
    pub nid: u64,
    pub file_type: FileType,
    pub name: String,
}

/// A file node in the EROFS image.
#[derive(Debug, Clone)]
pub struct FileNode(NodeInner);

impl std::ops::Deref for FileNode {
    type Target = NodeInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A symbolic link node in the EROFS image.
#[derive(Debug, Clone)]
pub struct SymlinkNode(NodeInner);

impl std::ops::Deref for SymlinkNode {
    type Target = NodeInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A node in the EROFS image.
#[derive(Debug, Clone)]
pub enum Node {
    Directory(DirectoryNode),
    File(FileNode),
    Symlink(SymlinkNode),
}

impl Node {
    fn new(inner: NodeInner) -> Self {
        if inner.is_dir() {
            Node::Directory(DirectoryNode(inner))
        } else if inner.is_symlink() {
            Node::Symlink(SymlinkNode(inner))
        } else {
            Node::File(FileNode(inner))
        }
    }

    fn parse_compact(
        nid: u64,
        inode_offset: u64,
        format: InodeFormat,
        inode: format::InodeCompact,
        build_time_ns: u64,
        reader: &dyn Reader,
    ) -> Result<Self, ErofsError> {
        let data_union = InodeDataUnion::parse(inode.i_u, format);
        let mut inner = NodeInner {
            inode_offset,
            format,
            mode: inode.mode.get(),
            size: inode.size.get().into(),
            data_union,
            ino: inode.ino.get(),
            nid,
            link_count: inode.link_count.get().into(),
            uid: inode.uid.get().into(),
            gid: inode.gid.get().into(),
            mtime_ns: build_time_ns,
            xattr_icount: inode.xattr_icount.get(),
            compression_header: None,
        };
        if matches!(
            format.data_layout,
            InodeDataLayout::CompressedFull | InodeDataLayout::CompressedCompact
        ) {
            let map_header_offset = inner.map_header_offset()?;
            inner.compression_header = Some(CompressionHeader::read(reader, map_header_offset)?);
        }
        Ok(Self::new(inner))
    }

    fn parse_extended(
        nid: u64,
        inode_offset: u64,
        format: InodeFormat,
        inode: format::InodeExtended,
        reader: &dyn Reader,
    ) -> Result<Self, ErofsError> {
        let data_union = InodeDataUnion::parse(inode.i_u, format);
        let mtime_ns = inode
            .mtime
            .get()
            .checked_mul(1_000_000_000)
            .and_then(|t| t.checked_add(inode.mtime_ns.get().into()))
            .ok_or(ParsingError::Overflow)?;
        let mut inner = NodeInner {
            inode_offset,
            format,
            mode: inode.mode.get(),
            size: inode.size.get(),
            data_union,
            ino: inode.ino.get(),
            nid,
            link_count: inode.link_count.get(),
            uid: inode.uid.get(),
            gid: inode.gid.get(),
            mtime_ns,
            xattr_icount: inode.xattr_icount.get(),
            compression_header: None,
        };
        if matches!(
            format.data_layout,
            InodeDataLayout::CompressedFull | InodeDataLayout::CompressedCompact
        ) {
            let map_header_offset = inner.map_header_offset()?;
            inner.compression_header = Some(CompressionHeader::read(reader, map_header_offset)?);
        }
        Ok(Self::new(inner))
    }

    fn from_nid(
        nid: u64,
        meta_addr: u64,
        build_time_ns: u64,
        reader: &dyn Reader,
    ) -> Result<Self, ErofsError> {
        let node_offset =
            nid.checked_mul(format::INODE_SLOT_SIZE).ok_or(ParsingError::InvalidNid(nid))?;
        let inode_offset =
            meta_addr.checked_add(node_offset).ok_or(ParsingError::InvalidNid(nid))?;
        // Read the first 2 bytes to determine the inode format.
        let mut head = [0u8; 2];
        reader.read(inode_offset, &mut head)?;
        let format = InodeFormat::parse(u16::from_le_bytes(head))?;
        let node = match format.version {
            InodeVersion::Compact => Self::parse_compact(
                nid,
                inode_offset,
                format,
                reader.read_object(inode_offset)?,
                build_time_ns,
                reader,
            )?,
            InodeVersion::Extended => Self::parse_extended(
                nid,
                inode_offset,
                format,
                reader.read_object(inode_offset)?,
                reader,
            )?,
        };
        Ok(node)
    }
}

impl std::ops::Deref for Node {
    type Target = NodeInner;
    fn deref(&self) -> &Self::Target {
        match self {
            Node::Directory(d) => d,
            Node::File(f) => f,
            Node::Symlink(s) => s,
        }
    }
}

/// The representation of an extent's backing content.
#[derive(Debug, Clone, Copy)]
enum ExtentKind {
    /// Sparse extents are regions of zeros without physical backing.
    Sparse,
    /// Plain extents are uncompressed data stored at a fixed on-disk offset.
    Plain { byte_offset: u64 },
    /// Compressed extents are compressed data that require decompression to read.
    Compressed { block_addr: u32 },
}

/// A compression extent is a logical, unaligned region of a file that maps to a single physical
/// cluster.
#[derive(Debug, Clone, Copy)]
struct CompressionExtent {
    logical_start: u64,
    logical_len: u32,
    kind: ExtentKind,
}

/// The filesystem implementation for an EROFS image.
pub struct ErofsFilesystem {
    reader: Arc<dyn Reader>,
    feature_incompat: FeatureIncompat,
    block_size: u64,
    meta_addr: u64,
    xattr_addr: u64,
    root_node: DirectoryNode,
    total_bytes: u64,
    total_inodes: u64,
    build_time_ns: u64,
}

impl ErofsFilesystem {
    /// Creates a new filesystem instance for an EROFS image from a reader.
    pub fn new(reader: Arc<dyn Reader>) -> Result<Self, ErofsError> {
        let (super_block, feature_incompat) = Self::parse_superblock(&reader)?;
        let block_size = 1u64 << super_block.block_size_bits;
        let meta_block_addr = super_block.meta_block_addr.get().into();
        let meta_addr = block_size.checked_mul(meta_block_addr).ok_or(ParsingError::Overflow)?;
        let total_inodes = super_block.inode_count.get();
        let build_time_ns = super_block
            .epoch
            .get()
            .checked_mul(1_000_000_000)
            .and_then(|t| t.checked_add(super_block.fixed_nsec.get().into()))
            .ok_or(ParsingError::Overflow)?;
        let total_bytes = (super_block.blocks.get() as u64) * block_size;
        let xattr_block_addr = super_block.xattr_block_addr.get().into();
        let xattr_addr = block_size.checked_mul(xattr_block_addr).ok_or(ParsingError::Overflow)?;
        let root_nid = super_block.root_nid.get().into();
        let root_node = match Node::from_nid(root_nid, meta_addr, build_time_ns, &reader)? {
            Node::Directory(node) => node,
            _ => return Err(ParsingError::InvalidRootNode.into()),
        };
        Ok(Self {
            reader,
            feature_incompat,
            block_size,
            meta_addr,
            xattr_addr,
            root_node,
            total_bytes,
            total_inodes,
            build_time_ns,
        })
    }

    /// Returns the feature incompat flags of the EROFS image.
    pub fn feature_incompat(&self) -> FeatureIncompat {
        self.feature_incompat
    }

    fn parse_superblock(
        reader: &dyn Reader,
    ) -> Result<(format::SuperBlock, FeatureIncompat), ErofsError> {
        let sb: format::SuperBlock = reader.read_object(format::SUPERBLOCK_OFFSET)?;
        if sb.magic.get() != format::EROFS_MAGIC {
            return Err(ParsingError::InvalidSuperBlockMagic(sb.magic.get()).into());
        }
        // The max block size that can be made by tooling is 4096 right now, and the specified
        // minimum is 512, so make sure we are in that window.
        if sb.block_size_bits < 9 || sb.block_size_bits > 12 {
            return Err(ParsingError::InvalidBlockSizeBits(sb.block_size_bits).into());
        }
        // TODO(https://fxbug.dev/479841115): Handle more feature_compat flags.
        let feature_compat = FeatureCompat::from_bits_truncate(sb.feature_compat.get());
        if feature_compat.contains(FeatureCompat::SB_CHKSUM) {
            Self::check_superblock_checksum(reader, &sb)?;
        }
        let incompat_raw = sb.feature_incompat.get();
        let incompat = FeatureIncompat::from_bits(incompat_raw).ok_or(
            ErofsError::UnsupportedFeatureIncompat(incompat_raw, FeatureIncompat::all().bits()),
        )?;
        Ok((sb, incompat))
    }

    fn check_superblock_checksum(
        reader: &dyn Reader,
        sb: &format::SuperBlock,
    ) -> Result<(), ErofsError> {
        let block_size = 1usize << sb.block_size_bits;
        let len = block_size - (format::SUPERBLOCK_OFFSET as usize) % block_size;
        let mut buf = vec![0u8; len];
        reader.read(format::SUPERBLOCK_OFFSET, &mut buf)?;

        // Zero out checksum field, which is at a well-known offset off the superblock offset.
        buf[4..8].copy_from_slice(&[0u8; 4]);

        let crc = Crc::<u32>::new(&CRC_32_ISCSI);
        let checksum = crc.checksum(&buf);
        // Undo final bitwise inversion applied by the crc crate, as suggested by the EROFS docs
        // (https://erofs.docs.kernel.org/en/latest/ondisk/core_ondisk.html#superblock-checksum)
        let checksum = !checksum;

        if checksum != sb.checksum.get() {
            Err(ParsingError::ChecksumMismatch(sb.checksum.get(), checksum).into())
        } else {
            Ok(())
        }
    }

    /// Returns the block size of the EROFS image.
    pub fn block_size(&self) -> u64 {
        self.block_size
    }

    /// Returns the node with the given nid.
    pub fn node(&self, nid: u64) -> Result<Node, ErofsError> {
        Node::from_nid(nid, self.meta_addr, self.build_time_ns, &self.reader)
    }

    /// Returns the root node of the EROFS image.
    pub fn root_node(&self) -> DirectoryNode {
        self.root_node.clone()
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn total_inodes(&self) -> u64 {
        self.total_inodes
    }

    /// Reads the data of the given file node into a buffer.
    pub fn read_file_range(
        &self,
        node: &FileNode,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ErofsError> {
        self.read_node_range(&node.0, offset, buf)
    }

    /// Reads the target path of the given symlink node.
    pub fn read_symlink(&self, node: &SymlinkNode) -> Result<Vec<u8>, ErofsError> {
        let mut target = vec![0u8; node.size() as usize];
        let read_bytes = self.read_node_range(&node.0, 0, &mut target)?;
        target.truncate(read_bytes);
        Ok(target)
    }

    /// Read bytes from the node's data at an offset. The length of the read is determined by the
    /// length of the provided output buf. The data is written into that buf. Returns the number of
    /// bytes read.
    ///
    /// TODO(https://fxbug.dev/479841115): This is a traditional unix-y way of handling reads -
    /// potentially reading less data than asked for - but we should determine whether that fits
    /// our apis and tweak it if needed.
    fn read_node_range(
        &self,
        node: &NodeInner,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ErofsError> {
        if offset >= node.size {
            return Ok(0);
        }
        let read_len = std::cmp::min(buf.len() as u64, node.size - offset) as usize;
        let buf = &mut buf[..read_len];
        let block_size = self.block_size();

        match node.format.data_layout {
            InodeDataLayout::FlatPlain => {
                let read_offset = node.blkaddr_offset(block_size, offset)?;
                self.reader.read(read_offset, buf)?;
                Ok(read_len)
            }
            InodeDataLayout::FlatInline => {
                // A node will _only_ have the flat inline layout if it has a tail that that fits
                // inline after the inode, so we can assume any tail data is there.
                let full_blocks_len = (node.size / block_size) * block_size;
                let mut bytes_read = 0;

                if offset < full_blocks_len {
                    // If there are no full blocks and the full file is in the tail section, this
                    // check will never be true, so this is a valid use of the u value.
                    let current_read_len =
                        std::cmp::min(read_len as u64, full_blocks_len - offset) as usize;
                    let read_offset = node.blkaddr_offset(block_size, offset)?;
                    self.reader.read(read_offset, &mut buf[..current_read_len])?;
                    bytes_read += current_read_len;
                }

                if bytes_read < read_len {
                    let remaining_len = read_len - bytes_read;
                    let current_offset = offset + bytes_read as u64;
                    let inline_data_offset = node.metadata_end_offset()?;
                    let tail_offset = current_offset - full_blocks_len;
                    let tail_read_offset = inline_data_offset
                        .checked_add(tail_offset)
                        .ok_or(ParsingError::Overflow)?;
                    self.reader.read(tail_read_offset, &mut buf[bytes_read..])?;
                    bytes_read += remaining_len;
                }

                Ok(bytes_read)
            }
            InodeDataLayout::CompressedFull | InodeDataLayout::CompressedCompact => {
                self.read_compressed_range(node, offset, buf)
            }
        }
    }

    fn read_lcluster_entry(
        &self,
        node: &NodeInner,
        cluster_index: u64,
    ) -> Result<LClusterEntry, ErofsError> {
        match node.format.data_layout {
            InodeDataLayout::CompressedFull => {
                let index_table_offset = node.index_table_offset()?;
                let entry_offset = index_table_offset + cluster_index * 8;
                LClusterEntry::read_full_entry(self.reader.as_ref(), entry_offset)
            }
            InodeDataLayout::CompressedCompact => {
                self.read_compact_lcluster_entry(node, cluster_index)
            }
            _ => Err(ParsingError::UnexpectedInodeDataLayout(node.format.data_layout).into()),
        }
    }

    fn read_compact_lcluster_entry(
        &self,
        node: &NodeInner,
        cluster_index: u64,
    ) -> Result<LClusterEntry, ErofsError> {
        let pos = node.compact_entry_pos(self.block_size(), cluster_index)?;
        let pack = CompactPack::read(self.reader.as_ref(), &pos)?;
        let entry = pack.entry(pos.entry_index)?;

        if entry.cluster_type() == LClusterType::NonHead {
            let delta0 = if pos.entry_index + 1 != pos.entry_count() {
                entry.delta0()
            } else {
                if pos.entry_index == 0 {
                    return Err(ParsingError::CorruptedCompactClusterPack(
                        "missing preceding entry for delta0 inference",
                    )
                    .into());
                }
                let prev_entry = pack.entry(pos.entry_index - 1)?;
                if prev_entry.cluster_type() != LClusterType::NonHead {
                    1
                } else {
                    prev_entry.delta0() + 1
                }
            };

            Ok(LClusterEntry::NonHead { delta0 })
        } else {
            let base_pblk = pack.base_pblk()?;

            let mut entry_i = pos.entry_index;
            let mut nblk = 1u32;

            while entry_i > 0 {
                entry_i -= 1;
                let prev_entry = pack.entry(entry_i)?;
                if prev_entry.cluster_type() == LClusterType::NonHead {
                    let step = prev_entry.delta0() as usize;
                    if entry_i >= step {
                        entry_i -= step;
                        nblk += 1;
                    } else {
                        break;
                    }
                } else {
                    nblk += 1;
                }
            }

            let extent_start_offset = entry.extent_start_offset();
            let block_addr = if base_pblk == u32::MAX { 0 } else { base_pblk + nblk };
            Ok(LClusterEntry::Head(LClusterHead {
                cluster_type: entry.cluster_type(),
                block_addr,
                extent_start_offset,
            }))
        }
    }

    /// Resolves the HEAD logical cluster for the extent that contains the given logical cluster
    /// and returns its logical cluster index and head entry.
    fn resolve_head_lcluster(
        &self,
        node: &NodeInner,
        cluster_index: u64,
    ) -> Result<(u64, LClusterHead), ErofsError> {
        let entry = self.read_lcluster_entry(node, cluster_index)?;
        match entry {
            LClusterEntry::Head(head) => Ok((cluster_index, head)),
            LClusterEntry::NonHead { delta0 } => {
                let head_cluster_index = cluster_index
                    .checked_sub(delta0 as u64)
                    .ok_or(ParsingError::InvalidLClusterDelta { cluster_index, delta0 })?;
                let head_entry = self.read_lcluster_entry(node, head_cluster_index)?;
                match head_entry {
                    LClusterEntry::Head(head) => Ok((head_cluster_index, head)),
                    LClusterEntry::NonHead { .. } => {
                        Err(ParsingError::ExpectedHeadLCluster(head_cluster_index).into())
                    }
                }
            }
        }
    }

    /// Finds the HEAD logical cluster containing the given `logical_offset`, stepping back by one
    /// cluster if `logical_offset` is before the cluster's offset. Returns the head cluster index,
    /// the head entry, and its logical start byte offset.
    fn find_head_cluster(
        &self,
        node: &NodeInner,
        logical_offset: u64,
    ) -> Result<(u64, LClusterHead, u64), ErofsError> {
        let lcluster_size = self.block_size();
        let cluster_index = logical_offset / lcluster_size;

        let (mut head_cluster_index, mut head) = self.resolve_head_lcluster(node, cluster_index)?;
        let mut logical_start =
            head.logical_start(head_cluster_index, lcluster_size).ok_or(ParsingError::Overflow)?;

        if logical_offset < logical_start {
            let prev_cluster_index =
                head_cluster_index.checked_sub(1).ok_or(ParsingError::InvalidClusterOffset {
                    offset: logical_offset,
                    cluster_start: logical_start,
                })?;
            (head_cluster_index, head) = self.resolve_head_lcluster(node, prev_cluster_index)?;
            logical_start = head
                .logical_start(head_cluster_index, lcluster_size)
                .ok_or(ParsingError::Overflow)?;
        }

        Ok((head_cluster_index, head, logical_start))
    }

    /// Maps a logical offset in a compressed file to the logical compression extent that contains
    /// it. This extent contains the metadata for this section of compressed data, and how to
    /// decompress it.
    fn get_extent_at(
        &self,
        node: &NodeInner,
        logical_offset: u64,
    ) -> Result<CompressionExtent, ErofsError> {
        let block_size = self.block_size();
        let lcluster_size = block_size;

        let (head_cluster_index, head, logical_start) =
            self.find_head_cluster(node, logical_offset)?;

        let mut logical_end = node.size;
        let max_cluster_index = node.total_lclusters(lcluster_size);
        for next_cluster_index in (head_cluster_index + 1)..max_cluster_index {
            let next_entry = self.read_lcluster_entry(node, next_cluster_index)?;
            if let LClusterEntry::Head(next_head) = next_entry {
                if let Some(next_start) = next_head.logical_start(next_cluster_index, lcluster_size)
                {
                    logical_end = next_start;
                    break;
                }
            }
        }

        let logical_len =
            logical_end.checked_sub(logical_start).ok_or(ParsingError::Overflow)? as u32;

        let kind = if head.block_addr == 0 {
            ExtentKind::Sparse
        } else if head.is_plain() {
            ExtentKind::Plain { byte_offset: head.block_addr as u64 * block_size }
        } else {
            ExtentKind::Compressed { block_addr: head.block_addr }
        };

        Ok(CompressionExtent { logical_start, logical_len, kind })
    }

    /// Reads a range of bytes from a compressed file node, decompressing as needed. This method is
    /// intended to be called by read_node_range. Use that for general reading to handle all
    /// possible data layouts.
    fn read_compressed_range(
        &self,
        node: &NodeInner,
        mut offset: u64,
        mut buf: &mut [u8],
    ) -> Result<usize, ErofsError> {
        // This value is checked and tweaked as needed by read_node_range.
        let read_len = buf.len();
        let block_size = self.block_size();

        while !buf.is_empty() {
            let extent = self.get_extent_at(node, offset)?;
            let offset_in_cluster = (offset - extent.logical_start) as usize;
            let available = extent.logical_len as usize - offset_in_cluster;
            let copy_len = std::cmp::min(buf.len(), available);
            let (head, tail) = buf.split_at_mut(copy_len);

            match &extent.kind {
                ExtentKind::Sparse => {
                    head.fill(0);
                }
                ExtentKind::Plain { byte_offset: disk_offset } => {
                    self.reader.read(*disk_offset + offset_in_cluster as u64, head)?;
                }
                ExtentKind::Compressed { block_addr } => {
                    let mut compressed_buf = vec![0u8; block_size as usize];
                    self.reader.read(*block_addr as u64 * block_size, &mut compressed_buf)?;
                    let margin = if self.feature_incompat.contains(FeatureIncompat::ZERO_PADDING) {
                        compressed_buf.iter().position(|&b| b != 0).unwrap_or(0)
                    } else {
                        0
                    };
                    let compressed_data = &compressed_buf[margin..];

                    if offset_in_cluster == 0 && copy_len == extent.logical_len as usize {
                        lz4::decompress_into(compressed_data, head)
                            .map_err(|e| ParsingError::DecompressionFailed(e))?;
                    } else {
                        let mut decompressed_buf = vec![0u8; extent.logical_len as usize];
                        lz4::decompress_into(compressed_data, &mut decompressed_buf)
                            .map_err(|e| ParsingError::DecompressionFailed(e))?;
                        head.copy_from_slice(
                            &decompressed_buf[offset_in_cluster..offset_in_cluster + copy_len],
                        );
                    }
                }
            }

            buf = tail;
            offset += copy_len as u64;
        }

        Ok(read_len)
    }

    /// Read a number of entries from a directory, starting at entry_offset. Will retrieve up to
    /// the number of entries in the directory or the size of the provided buffer, returning the
    /// number of entries filled in the buffer. If there are less filled entries then the number of
    /// entry slots provided in the buffer, there are no more entries in this directory. Entries
    /// are sorted lexicographically. Reads past the end of the number of entries will return zero
    /// entries filled.
    ///
    /// TODO(https://fxbug.dev/479841115): It is possible for directories to omit their "." entries
    /// in erofs, and in that case there is a flag marking it and we are expected to synthesize it.
    /// Parse that flag and implement it.
    /// TODO(https://fxbug.dev/479841115): This API is slightly awkward to hold. We should consider
    /// making it an iterator interface.
    pub fn read_directory(
        &self,
        node: &DirectoryNode,
        mut entry_offset: usize,
        entries: &mut [DirectoryEntry],
    ) -> Result<usize, ErofsError> {
        let block_size = self.block_size();
        let block_size_usize: usize = block_size as usize;
        let mut entries_filled = 0;
        let mut current_entry_index = 0;
        let mut block_data = vec![0u8; block_size_usize];

        for block in 0.. {
            let base_offset = block * block_size;
            let bytes_read = self.read_node_range(&node.0, base_offset, &mut block_data)?;
            if bytes_read < format::DIRENT_SIZE {
                // We must be done if there wasn't enough data left for another dirent.
                return Ok(entries_filled);
            }
            block_data[bytes_read..].fill(0);

            // Get the first dirent in the block to calculate the number of entries.
            let (dirent0, _) = zerocopy::Ref::<&[u8], format::Dirent>::from_prefix(&block_data)
                .map_err(|_| ParsingError::InvalidDirectoryEntry)?;
            let nameoff0 = dirent0.nameoff.get() as usize;
            if nameoff0 < format::DIRENT_SIZE || nameoff0 >= block_size_usize {
                return Err(ParsingError::InvalidDirectoryEntry.into());
            }
            let entry_count = nameoff0 / format::DIRENT_SIZE;

            // Check if the offset we want is even in this block.
            if current_entry_index + entry_count <= entry_offset {
                current_entry_index += entry_count;
                continue;
            }

            // Get all the dirents and make sure the nameoffs won't cause out of bounds errors.
            let dirents_raw = block_data
                .get(..entry_count * format::DIRENT_SIZE)
                .ok_or(ParsingError::InvalidDirectoryEntry)?;
            let dirents: &[format::Dirent] =
                &*zerocopy::Ref::<&[u8], [format::Dirent]>::from_bytes(dirents_raw)
                    .map_err(|_| ParsingError::InvalidDirectoryEntry)?;

            let block_entry_offset = entry_offset - current_entry_index;
            let space = entries.len() - entries_filled;
            let block_entry_end = std::cmp::min(
                entry_count,
                block_entry_offset.checked_add(space).ok_or(ParsingError::Overflow)?,
            );

            for i in block_entry_offset..block_entry_end {
                let last_entry = i + 1 == entry_count;
                let nameoff = dirents[i].nameoff.get() as usize;

                let name_bytes = if last_entry {
                    // For the last entry, it ends at the end of the block or is null-terminated.
                    // Since block_data is padded with nulls, we can just split by 0.
                    let name_data =
                        block_data.get(nameoff..).ok_or(ParsingError::InvalidDirectoryEntry)?;
                    name_data.split(|&x| x == 0).next().unwrap()
                } else {
                    let nameoff_next = dirents[i + 1].nameoff.get() as usize;
                    block_data
                        .get(nameoff..nameoff_next)
                        .ok_or(ParsingError::InvalidDirectoryEntry)?
                };

                let name = std::str::from_utf8(name_bytes)
                    .map_err(|e| ParsingError::InvalidDirectoryEntryName(e))?
                    .to_string();
                entries[entries_filled] = DirectoryEntry {
                    nid: dirents[i].nid.get(),
                    file_type: dirents[i].file_type.try_into()?,
                    name,
                };
                entries_filled += 1;
                if entries_filled == entries.len() {
                    return Ok(entries_filled);
                }
            }

            current_entry_index =
                current_entry_index.checked_add(entry_count).ok_or(ParsingError::Overflow)?;
            entry_offset = current_entry_index;
        }

        Ok(entries_filled)
    }

    /// Looks up a node by name in a directory.
    pub fn lookup(&self, dir: &DirectoryNode, name: &str) -> Result<Option<Node>, ErofsError> {
        let mut entry_offset = 0;
        let mut buffer = vec![DirectoryEntry::default(); 16];

        loop {
            let filled = self.read_directory(dir, entry_offset, &mut buffer)?;
            for i in 0..filled {
                if buffer[i].name == name {
                    let node = self.node(buffer[i].nid)?;
                    return Ok(Some(node));
                }
            }
            if filled < buffer.len() {
                break;
            }
            entry_offset += filled;
        }

        Ok(None)
    }

    /// Returns an iterator over the xattr entry headers for a node.
    pub fn iter_xattrs<'a>(&'a self, node: &NodeInner) -> Result<XattrIterator<'a>, ErofsError> {
        if node.xattr_icount == 0 {
            return Ok(XattrIterator {
                reader: self.reader.as_ref(),
                xattr_addr: self.xattr_addr,
                shared_ids: Vec::new(),
                inline_offset: 0,
                inline_end: 0,
            });
        }
        let xattr_metadata_size = node.inline_xattr_size();
        let xattr_metadata_start =
            node.inode_offset().checked_add(node.metadata_size()).ok_or(ParsingError::Overflow)?;

        // Read the inline xattr header to get the details on the extended attributes for this node
        let header: format::XattrInlineBodyHeader =
            self.reader.read_object(xattr_metadata_start)?;
        let shared_count = header.shared_count as usize;

        let shared_ids_size = shared_count as u64 * 4;
        let inline_entries_start = xattr_metadata_start + 12 + shared_ids_size;
        let inline_end = xattr_metadata_start + xattr_metadata_size;

        if inline_entries_start > inline_end {
            return Err(ParsingError::XattrEntryOutOfBounds.into());
        }

        let shared_ids = if shared_count > 0 {
            let mut ids = vec![LEU32::ZERO; shared_count];
            self.reader.read(xattr_metadata_start + 12, ids.as_mut_bytes())?;
            ids
        } else {
            Vec::new()
        };

        Ok(XattrIterator {
            reader: self.reader.as_ref(),
            xattr_addr: self.xattr_addr,
            shared_ids,
            inline_offset: inline_entries_start,
            inline_end,
        })
    }

    /// List all xattr names for a given node.
    pub fn list_xattrs(&self, node: &NodeInner) -> Result<Vec<Vec<u8>>, ErofsError> {
        let mut names = Vec::new();
        for entry in self.iter_xattrs(node)? {
            let entry = entry?;
            names.push(entry.read_name(self.reader.as_ref())?);
        }
        Ok(names)
    }

    /// Get the value of a specific xattr for a given node.
    pub fn get_xattr(&self, node: &NodeInner, name: &[u8]) -> Result<Option<Vec<u8>>, ErofsError> {
        for entry in self.iter_xattrs(node)? {
            let entry = entry?;
            if entry.matches_name(self.reader.as_ref(), name)? {
                return Ok(Some(entry.read_value(self.reader.as_ref())?));
            }
        }
        Ok(None)
    }
}

/// An iterator over xattr entry headers for an inode.
pub struct XattrIterator<'a> {
    reader: &'a dyn Reader,
    xattr_addr: u64,
    shared_ids: Vec<LEU32>,
    inline_offset: u64,
    inline_end: u64,
}

impl XattrIterator<'_> {
    fn next_inner(&mut self) -> Result<Option<XattrEntryHeader>, ErofsError> {
        if let Some(shared_id) = self.shared_ids.pop() {
            let shared_entry_offset = self.xattr_addr + (shared_id.get() as u64 * 4);
            if self.shared_ids.is_empty() {
                self.shared_ids = Vec::new();
            }
            return Ok(Some(XattrEntryHeader::parse(self.reader, shared_entry_offset)?));
        }

        if self.inline_offset < self.inline_end {
            if self.inline_offset + 4 > self.inline_end {
                return Err(ParsingError::XattrEntryOutOfBounds.into());
            }

            let header = XattrEntryHeader::parse(self.reader, self.inline_offset)?;
            let next_offset = self
                .inline_offset
                .checked_add(header.entry_aligned_size)
                .ok_or(ParsingError::Overflow)?;
            if next_offset > self.inline_end {
                return Err(ParsingError::XattrEntryOutOfBounds.into());
            }
            self.inline_offset = next_offset;
            Ok(Some(header))
        } else {
            Ok(None)
        }
    }
}

impl Iterator for XattrIterator<'_> {
    type Item = Result<XattrEntryHeader, ErofsError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_inner() {
            // Throw out the rest of the values if we encounter an error parsing the extended
            // attributes. Since most of the errors are related to overflows and math issues, there
            // is no safe way to recover for future attributes as the locations on disk are all
            // relative to each other.
            Err(e) => {
                self.shared_ids = Vec::new();
                self.inline_offset = self.inline_end;
                Some(Err(e))
            }
            Ok(None) => None,
            Ok(Some(x)) => Some(Ok(x)),
        }
    }
}

/// A parsed representation of an EROFS xattr entry record header.
#[derive(Debug, Clone, Copy)]
pub struct XattrEntryHeader {
    pub offset: u64,
    pub prefix: &'static [u8],
    pub name_index: u8,
    pub name_len: usize,
    pub value_size: usize,
    pub entry_aligned_size: u64,
}

impl XattrEntryHeader {
    /// Read and validate an xattr entry record header from the reader.
    pub fn parse(reader: &dyn Reader, offset: u64) -> Result<Self, ErofsError> {
        let entry: format::XattrEntry = reader.read_object(offset)?;
        let prefix = Self::get_xattr_prefix(entry.name_index)?;
        let name_len = entry.name_len as usize;
        let value_size = entry.value_size.get() as usize;

        let entry_aligned_size = 4usize
            .checked_add(name_len)
            .and_then(|s| s.checked_add(value_size))
            .and_then(|s| s.checked_next_multiple_of(4))
            .ok_or(ParsingError::Overflow)? as u64;

        Ok(Self {
            offset,
            prefix,
            name_index: entry.name_index,
            name_len,
            value_size,
            entry_aligned_size,
        })
    }

    /// Check if this xattr entry matches the given full attribute name (prefix + suffix).
    pub fn matches_name(&self, reader: &dyn Reader, name: &[u8]) -> Result<bool, ReaderError> {
        let Some(suffix) = name.strip_prefix(self.prefix) else {
            return Ok(false);
        };
        if suffix.len() != self.name_len {
            return Ok(false);
        }
        if self.name_len == 0 {
            // Implies suffix.len() is also zero because of the previous check.
            return Ok(true);
        }
        let mut buf = vec![0u8; self.name_len];
        reader.read(self.offset + 4, &mut buf)?;
        Ok(buf == suffix)
    }

    /// Read the name of this xattr entry (prefix + suffix).
    pub fn read_name(&self, reader: &dyn Reader) -> Result<Vec<u8>, ReaderError> {
        let mut name_bytes = Vec::with_capacity(self.prefix.len() + self.name_len);
        name_bytes.extend_from_slice(self.prefix);
        if self.name_len > 0 {
            name_bytes.resize(self.prefix.len() + self.name_len, 0);
            reader.read(self.offset + 4, &mut name_bytes[self.prefix.len()..])?;
        }
        Ok(name_bytes)
    }

    /// Read the value payload for this entry.
    pub fn read_value(&self, reader: &dyn Reader) -> Result<Vec<u8>, ReaderError> {
        let mut value_bytes = vec![0u8; self.value_size];
        reader.read(self.offset + 4 + self.name_len as u64, &mut value_bytes)?;
        Ok(value_bytes)
    }

    /// Read both key name and value payload for this entry.
    pub fn read_payload(&self, reader: &dyn Reader) -> Result<(Vec<u8>, Vec<u8>), ReaderError> {
        let name = self.read_name(reader)?;
        let value = self.read_value(reader)?;
        Ok((name, value))
    }

    fn get_xattr_prefix(index: u8) -> Result<&'static [u8], ParsingError> {
        match index {
            1 => Ok(b"user."),
            2 => Ok(b"system.posix_acl_access"),
            3 => Ok(b"system.posix_acl_default"),
            4 => Ok(b"trusted."),
            6 => Ok(b"security."),
            _ => Err(ParsingError::InvalidXattrNamespace(index)),
        }
    }
}

/// The version of the on-disk format of the inode. Can be either 32-byte compact or 64-byte
/// extended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeVersion {
    Compact,
    Extended,
}

/// The layout of the data portion of the inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeDataLayout {
    /// The data union is interpreted as a block address. The data for this inode is stored in
    /// consecutive blocks starting from that block address.
    FlatPlain,
    /// Compressed inode with non-compact indexes. This is a legacy metadata layout, by default
    /// erofs images use CompressedCompact for compressed nodes.
    CompressedFull,
    /// The data union is interpreted as a block address. The data for this inode is stored in
    /// consecutive blocks starting from that block address, except for the tail of the data which
    /// is stored immediately following this metadata. If the whole tail is inlined, the data union
    /// is unused and doesn't matter. For this to be used, the data _must_ have a tail section that
    /// fits within the current metadata block.
    FlatInline,
    /// Compressed inode with compact indexes.
    CompressedCompact,
}

/// The format of the inode, containing the version and data layout.
#[derive(Debug, Clone, Copy)]
pub struct InodeFormat {
    pub version: InodeVersion,
    pub data_layout: InodeDataLayout,
}

impl InodeFormat {
    /// Parse the inode format from the given format value.
    pub fn parse(format: u16) -> Result<Self, ParsingError> {
        let version =
            if format & 0x1 == 0 { InodeVersion::Compact } else { InodeVersion::Extended };
        let data_layout_raw = (format >> 1) & 0x7;
        let data_layout = match data_layout_raw {
            0 => InodeDataLayout::FlatPlain,
            1 => InodeDataLayout::CompressedFull,
            2 => InodeDataLayout::FlatInline,
            3 => InodeDataLayout::CompressedCompact,
            _ => return Err(ParsingError::InvalidInodeDataLayout(data_layout_raw)),
        };
        Ok(Self { version, data_layout })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LClusterType {
    Plain,
    Head1,
    NonHead,
    Head2,
}

impl LClusterType {
    pub fn is_head(&self) -> bool {
        matches!(self, Self::Head1 | Self::Head2 | Self::Plain)
    }
}

impl TryFrom<u16> for LClusterType {
    type Error = ParsingError;

    fn try_from(advise: u16) -> Result<Self, Self::Error> {
        match advise & 3 {
            0 => Ok(Self::Plain),
            1 => Ok(Self::Head1),
            2 => Ok(Self::NonHead),
            3 => Ok(Self::Head2),
            other => Err(ParsingError::InvalidLClusterType(other)),
        }
    }
}

/// A head lcluster is one where the data for a particular extent starts. The logical data
/// potentially starts at an unaligned address within this lcluster, described by
/// [`extent_start_offset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LClusterHead {
    /// The type of this cluster. Will be Head1, Head2, or Plain for this struct.
    pub cluster_type: LClusterType,
    /// The physical block address where the data lives for the extent described by this set of
    /// entries.
    pub block_addr: u32,
    /// The offset into this lcluster where the data it is describing actually starts. Anything
    /// before this offset is actually from the previous extent, so when looking at these entries,
    /// if the requested read offset is before this start offset, the read logic needs to walk back
    /// one extent to find the relevant data.
    pub extent_start_offset: u16,
}

impl LClusterHead {
    pub fn is_plain(&self) -> bool {
        self.cluster_type == LClusterType::Plain
    }

    pub fn logical_start(&self, cluster_index: u64, lcluster_size: u64) -> Option<u64> {
        cluster_index.checked_mul(lcluster_size)?.checked_add(self.extent_start_offset as u64)
    }
}

/// An entry describing a single logical cluster, which most often corresponds with a single
/// logical, uncompressed block. These entries build a map for where to find the data in the
/// compressed physical blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LClusterEntry {
    /// A head cluster. See LClusterHead.
    Head(LClusterHead),
    /// A nonhead cluster. These are blocks of data contained within the extent described by the
    /// most recent head lcluster.
    NonHead {
        /// The distance back to the most recent head lcluster.
        delta0: u16,
    },
}

impl LClusterEntry {
    /// Parse a single lcluster entry. This is for the CompressedFull data layout, for the
    /// CompressedCompact data layout, see [`read_compact_lcluster_entry`].
    pub fn read_full_entry(reader: &dyn Reader, offset: u64) -> Result<Self, ErofsError> {
        let raw: format::LClusterIndex = reader.read_object(offset)?;
        let advise = raw.advisory_flags.get();
        let cluster_type = LClusterType::try_from(advise)?;
        let extent_start_offset = raw.extent_start_offset.get();

        if cluster_type.is_head() {
            let block_addr = u32::from_le_bytes(raw.data_union);
            Ok(Self::Head(LClusterHead { cluster_type, block_addr, extent_start_offset }))
        } else {
            let delta0 = u16::from_le_bytes([raw.data_union[0], raw.data_union[1]]);
            Ok(Self::NonHead { delta0 })
        }
    }
}

/// The layout and geometry of a compact logical cluster index pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactPackLayout {
    /// 8-byte pack: 2 entries (16 bits each) followed by 4-byte base_pblk. Used for 4B entries.
    Pack4B,
    /// 32-byte pack: 16 entries (14 bits each) followed by 4-byte base_pblk. Used for 2B entries.
    Pack2B,
}

impl CompactPackLayout {
    pub const fn pack_size(&self) -> u64 {
        match self {
            Self::Pack4B => 8,
            Self::Pack2B => 32,
        }
    }

    pub const fn entry_count(&self) -> usize {
        match self {
            Self::Pack4B => 2,
            Self::Pack2B => 16,
        }
    }

    pub const fn encode_bits(&self) -> usize {
        match self {
            Self::Pack4B => 16,
            Self::Pack2B => 14,
        }
    }
}

/// Position and layout information for a compact logical cluster index entry on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactEntry {
    /// On-disk start offset of the pack containing this entry.
    pub pack_offset_bytes: u64,
    /// Layout of the pack (Pack4B or Pack2B).
    pub layout: CompactPackLayout,
    /// Index of this entry within the pack.
    pub entry_index: usize,
}

impl CompactEntry {
    pub fn pack_size(&self) -> u64 {
        self.layout.pack_size()
    }

    pub fn entry_count(&self) -> usize {
        self.layout.entry_count()
    }

    pub fn encode_bits(&self) -> usize {
        self.layout.encode_bits()
    }
}

/// An on-disk compact logical cluster index pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactPack {
    layout: CompactPackLayout,
    data: [u8; 32],
}

impl CompactPack {
    pub fn read(reader: &dyn Reader, pos: &CompactEntry) -> Result<Self, ErofsError> {
        let mut data = [0u8; 32];
        let pack_size = pos.pack_size() as usize;
        reader.read(pos.pack_offset_bytes, &mut data[..pack_size])?;
        Ok(Self { layout: pos.layout, data })
    }

    pub fn from_bytes(layout: CompactPackLayout, buf: &[u8]) -> Result<Self, ParsingError> {
        let min_size = layout.pack_size() as usize;
        if buf.len() < min_size {
            return Err(ParsingError::CompactPackOutOfBounds {
                byte_offset: 0,
                pack_size: buf.len(),
            });
        }
        let mut data = [0u8; 32];
        data[..min_size].copy_from_slice(&buf[..min_size]);
        Ok(Self { layout, data })
    }

    pub fn base_pblk(&self) -> Result<u32, ParsingError> {
        let size = self.layout.pack_size() as usize;
        let bytes: [u8; 4] = self
            .data
            .get(size - 4..size)
            .ok_or(ParsingError::CompactPackOutOfBounds { byte_offset: size - 4, pack_size: size })?
            .try_into()
            .map_err(|_| ParsingError::CompactPackOutOfBounds {
                byte_offset: size - 4,
                pack_size: size,
            })?;
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn entry(&self, entry_index: usize) -> Result<CompactPackEntry, ParsingError> {
        let pack_size = self.layout.pack_size() as usize;
        let bit_offset = self.layout.encode_bits() * entry_index;
        let byte_offset = bit_offset / 8;
        let bit_shift = bit_offset & 7;
        let bytes: [u8; 4] = self
            .data
            .get(byte_offset..byte_offset + 4)
            .ok_or(ParsingError::CompactPackOutOfBounds { byte_offset, pack_size })?
            .try_into()
            .map_err(|_| ParsingError::CompactPackOutOfBounds { byte_offset, pack_size })?;
        let v = u32::from_le_bytes(bytes) >> bit_shift;
        let mask = (1u32 << COMPACT_ENTRY_LOBITS) - 1;
        let data = (v & mask) as u16;
        let type_raw = ((v >> COMPACT_ENTRY_LOBITS) & 3) as u16;
        let cluster_type = LClusterType::try_from(type_raw)?;
        Ok(CompactPackEntry { data, cluster_type })
    }
}

/// An individual entry decoded from a compact logical cluster index pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactPackEntry {
    /// The entry bytes store either the extent start offset, for HEAD type clusters, or the
    /// distance back to the previous head for nonhead clusters.
    data: u16,
    /// The type of this cluster.
    cluster_type: LClusterType,
}

impl CompactPackEntry {
    pub fn cluster_type(&self) -> LClusterType {
        self.cluster_type
    }

    /// For HEAD or PLAIN entries, interpret the data as the start offset of the extent within in
    /// the logical cluster.
    pub fn extent_start_offset(&self) -> u16 {
        self.data
    }

    /// For NONHEAD entries, returns the raw delta0 distance value.
    pub fn delta0(&self) -> u16 {
        self.data
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressionHeader {
    pub advise: CompressionAdvise,
}

impl CompressionHeader {
    pub fn read(reader: &dyn Reader, offset: u64) -> Result<Self, ErofsError> {
        let raw: format::CompressionMapHeader = reader.read_object(offset)?;
        if raw.algorithm_type != 0 {
            return Err(ParsingError::UnexpectedCompressionAlgorithm(raw.algorithm_type).into());
        }
        let advise = CompressionAdvise::from_bits_truncate(raw.advisory_flags.get());
        Ok(Self { advise })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readers::VecReader;
    use std::fs;
    use test_case::test_case;
    use zerocopy::byteorder::little_endian::{U16 as LEU16, U32 as LEU32, U64 as LEU64};

    #[test]
    fn test_lcluster_type_and_parsing() {
        assert_eq!(LClusterType::try_from(0).unwrap(), LClusterType::Plain);
        assert_eq!(LClusterType::try_from(1).unwrap(), LClusterType::Head1);
        assert_eq!(LClusterType::try_from(2).unwrap(), LClusterType::NonHead);
        assert_eq!(LClusterType::try_from(3).unwrap(), LClusterType::Head2);
        assert!(LClusterType::Head1.is_head());
        assert!(LClusterType::Plain.is_head());
        assert!(!LClusterType::NonHead.is_head());

        // Parse Head entry
        let raw_head = format::LClusterIndex {
            advisory_flags: LEU16::new(1),
            extent_start_offset: LEU16::new(128),
            data_union: 0x1000u32.to_le_bytes(),
        };
        let reader = VecReader::new(raw_head.as_bytes().to_vec());
        let parsed_head = LClusterEntry::read_full_entry(&reader, 0).unwrap();
        assert_eq!(
            parsed_head,
            LClusterEntry::Head(LClusterHead {
                cluster_type: LClusterType::Head1,
                block_addr: 0x1000,
                extent_start_offset: 128,
            })
        );

        // Parse NonHead entry
        let raw_nonhead = format::LClusterIndex {
            advisory_flags: LEU16::new(2),
            extent_start_offset: LEU16::new(0),
            data_union: [4, 0, 0, 0],
        };
        let reader_nonhead = VecReader::new(raw_nonhead.as_bytes().to_vec());
        let parsed_nonhead = LClusterEntry::read_full_entry(&reader_nonhead, 0).unwrap();
        assert_eq!(parsed_nonhead, LClusterEntry::NonHead { delta0: 4 });
    }

    #[test]
    fn test_compact_entry_pos_and_index_end_offset() {
        let block_size = 4096u64;
        let num_lclusters = 10u64;
        let node = NodeInner {
            inode_offset: 4096,
            format: InodeFormat {
                version: InodeVersion::Compact,
                data_layout: InodeDataLayout::CompressedCompact,
            },
            mode: 0o100644,
            size: num_lclusters * block_size,
            data_union: InodeDataUnion::CompressedBlocks(0),
            ino: 1,
            nid: 1,
            link_count: 1,
            uid: 0,
            gid: 0,
            mtime_ns: 0,
            xattr_icount: 0,
            compression_header: Some(CompressionHeader { advise: CompressionAdvise::empty() }),
        };

        // map_header_offset = (4096 + 32).next_multiple_of(8) = 4128
        assert_eq!(node.map_header_offset().unwrap(), 4128);
        // index_table_offset = 4128 + 8 = 4136
        assert_eq!(node.index_table_offset().unwrap(), 4136);

        // Test compact_entry_pos for 4B entries (8-byte packs)
        let pos0 = node.compact_entry_pos(block_size, 0).unwrap();
        assert_eq!(pos0.pack_offset_bytes, 4136);
        assert_eq!(pos0.layout, CompactPackLayout::Pack4B);
        assert_eq!(pos0.pack_size(), 8);
        assert_eq!(pos0.entry_index, 0);
        assert_eq!(pos0.encode_bits(), 16);
        assert_eq!(pos0.entry_count(), 2);

        let pos1 = node.compact_entry_pos(block_size, 1).unwrap();
        assert_eq!(pos1.pack_offset_bytes, 4136);
        assert_eq!(pos1.layout, CompactPackLayout::Pack4B);
        assert_eq!(pos1.entry_index, 1);

        let pos2 = node.compact_entry_pos(block_size, 2).unwrap();
        assert_eq!(pos2.pack_offset_bytes, 4144);
        assert_eq!(pos2.layout, CompactPackLayout::Pack4B);
        assert_eq!(pos2.entry_index, 0);

        // Out of bounds lcn
        assert_eq!(
            node.compact_entry_pos(block_size, num_lclusters),
            Err(ParsingError::LClusterOutOfBounds { cluster_index: 10, total_lclusters: 10 })
        );

        // Missing compression header
        let mut no_header_node = node.clone();
        no_header_node.compression_header = None;
        assert_eq!(
            no_header_node.compact_entry_pos(block_size, 0),
            Err(ParsingError::MissingCompressionHeader)
        );

        // Unexpected layout (FlatPlain)
        let mut plain_node = node.clone();
        plain_node.format.data_layout = InodeDataLayout::FlatPlain;
        assert_eq!(
            plain_node.compact_entry_pos(block_size, 0),
            Err(ParsingError::UnexpectedInodeDataLayout(InodeDataLayout::FlatPlain))
        );
        assert_eq!(
            plain_node.index_end_offset(block_size),
            Err(ParsingError::UnexpectedInodeDataLayout(InodeDataLayout::FlatPlain))
        );

        // index_end_offset for 10 clusters (last cluster 9 is in pack 4168..4176)
        assert_eq!(node.index_end_offset(block_size).unwrap(), 4176);

        // Empty file with CompressedCompact layout
        let mut empty_compact_node = node.clone();
        empty_compact_node.size = 0;
        assert_eq!(empty_compact_node.index_end_offset(block_size).unwrap(), 4136);

        // CompressedFull layout
        let mut full_node = node.clone();
        full_node.format.data_layout = InodeDataLayout::CompressedFull;
        // index_table_offset = 4128 + 16 = 4144
        assert_eq!(full_node.index_table_offset().unwrap(), 4144);
        // index_end_offset = 4144 + 10 * 8 = 4224
        assert_eq!(full_node.index_end_offset(block_size).unwrap(), 4224);
    }

    #[test]
    fn test_unexpected_compression_algorithm() {
        let raw = format::CompressionMapHeader {
            reserved_1: [0; 2],
            inline_data_size: LEU16::new(0),
            advisory_flags: LEU16::new(0),
            algorithm_type: 2, // Non-zero (e.g. LZMA)
            lcluster_bits: 0,
        };
        let reader = VecReader::new(raw.as_bytes().to_vec());
        let result = CompressionHeader::read(&reader, 0);
        assert_eq!(result, Err(ErofsError::Parse(ParsingError::UnexpectedCompressionAlgorithm(2))));
    }

    #[test]
    fn test_compact_entry_decode() {
        // Pack buffer with 2 entries of 16 bits each (4 bytes total + 4 bytes base_pblk)
        // Entry 0: lo = 128 (0x0080), type = Head1 (1) -> raw = (1 << 12) | 128 = 0x1080
        // Entry 1: lo = 3, type = NonHead (2) -> raw = (2 << 12) | 3 = 0x2003
        // base_pblk: 0x00000020
        let entry1_raw = (2u16 << 12) | 3;
        let mut pack_buf = [0u8; 8];
        pack_buf[0..2].copy_from_slice(&0x1080u16.to_le_bytes());
        pack_buf[2..4].copy_from_slice(&entry1_raw.to_le_bytes());
        pack_buf[4..8].copy_from_slice(&0x00000020u32.to_le_bytes());

        let pack = CompactPack::from_bytes(CompactPackLayout::Pack4B, &pack_buf).unwrap();
        assert_eq!(pack.base_pblk().unwrap(), 0x20);

        let entry0 = pack.entry(0).unwrap();
        assert_eq!(entry0.extent_start_offset(), 128);
        assert_eq!(entry0.cluster_type(), LClusterType::Head1);

        let entry1 = pack.entry(1).unwrap();
        assert_eq!(entry1.cluster_type(), LClusterType::NonHead);
        assert_eq!(entry1.delta0(), 3);
    }

    fn create_synthetic_sparse_filesystem() -> (ErofsFilesystem, FileNode) {
        let block_size = 4096usize;
        let num_blocks = 8;
        let mut image = vec![0u8; num_blocks * block_size];

        // 1. Superblock at offset 1024
        let sb = format::SuperBlock {
            magic: LEU32::new(format::EROFS_MAGIC),
            checksum: LEU32::new(0),
            feature_compat: LEU32::new(0),
            block_size_bits: 12,
            sb_ext_slots: 0,
            root_nid: LEU16::new(0),
            inode_count: LEU64::new(2),
            epoch: LEU64::new(0),
            fixed_nsec: LEU32::new(0),
            blocks: LEU32::new(num_blocks as u32),
            meta_block_addr: LEU32::new(1),
            xattr_block_addr: LEU32::new(0),
            uuid: [0; 16],
            volume_name: [0; 16],
            feature_incompat: LEU32::new(0),
            available_compr_algs: LEU16::new(0),
            extra_devices: LEU32::new(0),
            dirblkbits: 0,
            reserved: [0; 37],
        };
        image[format::SUPERBLOCK_OFFSET as usize
            ..format::SUPERBLOCK_OFFSET as usize + std::mem::size_of::<format::SuperBlock>()]
            .copy_from_slice(sb.as_bytes());

        // 2. Root directory inode at nid 0 (offset 4096)
        let root_inode = format::InodeCompact {
            format: LEU16::new(0),
            xattr_icount: LEU16::new(0),
            mode: LEU16::new(0o040755),
            link_count: LEU16::new(2),
            size: LEU32::new(0),
            reserved_1: [0; 4],
            i_u: [0; 4],
            ino: LEU32::new(1),
            uid: LEU16::new(0),
            gid: LEU16::new(0),
            reserved_2: [0; 4],
        };
        image[4096..4096 + 32].copy_from_slice(root_inode.as_bytes());

        // 3. Compressed file inode at nid 1 (offset 4128)
        let file_size = 16384u32; // 4 clusters of 4096
        let file_inode = format::InodeCompact {
            format: LEU16::new((InodeDataLayout::CompressedFull as u16) << 1),
            xattr_icount: LEU16::new(0),
            mode: LEU16::new(0o100644),
            link_count: LEU16::new(1),
            size: LEU32::new(file_size),
            reserved_1: [0; 4],
            i_u: [0; 4],
            ino: LEU32::new(2),
            uid: LEU16::new(0),
            gid: LEU16::new(0),
            reserved_2: [0; 4],
        };
        image[4128..4128 + 32].copy_from_slice(file_inode.as_bytes());

        // 4. CompressionMapHeader at map_header_offset = 4160
        let map_header = format::CompressionMapHeader {
            reserved_1: [0; 2],
            inline_data_size: LEU16::new(0),
            advisory_flags: LEU16::new(0),
            algorithm_type: 0,
            lcluster_bits: 0,
        };
        image[4160..4160 + 8].copy_from_slice(map_header.as_bytes());

        // 5. LClusterIndex table at index_table_offset = 4160 + 16 = 4176
        // Cluster 0 (0..4096): Plain data pointing to Block 4
        let cluster0 = format::LClusterIndex {
            advisory_flags: LEU16::new(0),
            extent_start_offset: LEU16::new(0),
            data_union: 4u32.to_le_bytes(),
        };
        // Cluster 1 (4096..8192): Sparse hole (blkaddr = 0)
        let cluster1 = format::LClusterIndex {
            advisory_flags: LEU16::new(0),
            extent_start_offset: LEU16::new(0),
            data_union: 0u32.to_le_bytes(),
        };
        // Cluster 2 (8192..12288): Sparse hole (blkaddr = 0)
        let cluster2 = format::LClusterIndex {
            advisory_flags: LEU16::new(0),
            extent_start_offset: LEU16::new(0),
            data_union: 0u32.to_le_bytes(),
        };
        // Cluster 3 (12288..16384): Plain data pointing to Block 5
        let cluster3 = format::LClusterIndex {
            advisory_flags: LEU16::new(0),
            extent_start_offset: LEU16::new(0),
            data_union: 5u32.to_le_bytes(),
        };
        image[4176..4176 + 8].copy_from_slice(cluster0.as_bytes());
        image[4184..4184 + 8].copy_from_slice(cluster1.as_bytes());
        image[4192..4192 + 8].copy_from_slice(cluster2.as_bytes());
        image[4200..4200 + 8].copy_from_slice(cluster3.as_bytes());

        // 6. Data in Block 4 (offset 16384) and Block 5 (offset 20480)
        image[16384..16384 + 4096].fill(0xAA);
        image[20480..20480 + 4096].fill(0xBB);

        let reader = Arc::new(VecReader::new(image));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse synthetic fs");
        let node = fs.node(1).expect("failed to get node 1");
        let Node::File(file_node) = node else { panic!("expected file node") };

        (fs, file_node)
    }

    #[test]
    fn test_synthetic_sparse_extents() {
        let (fs, file_node) = create_synthetic_sparse_filesystem();

        // Cluster 0: Block extent (0..4096)
        let extent0 = fs.get_extent_at(&file_node.0, 0).unwrap();
        assert_eq!(extent0.logical_start, 0);
        assert_eq!(extent0.logical_len, 4096);
        match extent0.kind {
            ExtentKind::Plain { byte_offset: disk_offset } => {
                assert_eq!(disk_offset, 4 * 4096);
            }
            other => panic!("expected Plain extent at 0, got {:?}", other),
        }

        // Cluster 1: Sparse extent (4096..8192)
        let extent1 = fs.get_extent_at(&file_node.0, 4096).unwrap();
        assert_eq!(extent1.logical_start, 4096);
        assert_eq!(extent1.logical_len, 4096);
        assert!(matches!(extent1.kind, ExtentKind::Sparse));

        // Inside Cluster 1: Sparse extent at offset 5000
        let extent_mid = fs.get_extent_at(&file_node.0, 5000).unwrap();
        assert_eq!(extent_mid.logical_start, 4096);
        assert_eq!(extent_mid.logical_len, 4096);
        assert!(matches!(extent_mid.kind, ExtentKind::Sparse));

        // Cluster 2: Sparse extent (8192..12288)
        let extent2 = fs.get_extent_at(&file_node.0, 8192).unwrap();
        assert_eq!(extent2.logical_start, 8192);
        assert_eq!(extent2.logical_len, 4096);
        assert!(matches!(extent2.kind, ExtentKind::Sparse));

        // Cluster 3: Block extent (12288..16384)
        let extent3 = fs.get_extent_at(&file_node.0, 12288).unwrap();
        assert_eq!(extent3.logical_start, 12288);
        assert_eq!(extent3.logical_len, 4096);
        match extent3.kind {
            ExtentKind::Plain { byte_offset: disk_offset } => {
                assert_eq!(disk_offset, 5 * 4096);
            }
            other => panic!("expected Plain extent at 12288, got {:?}", other),
        }
    }

    #[test]
    fn test_synthetic_sparse_reads() {
        let (fs, file_node) = create_synthetic_sparse_filesystem();

        // 1. Full read across all extents (Plain -> Sparse -> Sparse -> Plain)
        let mut full_buf = vec![0u8; 16384];
        let bytes_read = fs.read_file_range(&file_node, 0, &mut full_buf).unwrap();
        assert_eq!(bytes_read, 16384);
        assert_eq!(&full_buf[0..4096], &[0xAA; 4096]);
        assert_eq!(&full_buf[4096..12288], &[0x00; 8192]);
        assert_eq!(&full_buf[12288..16384], &[0xBB; 4096]);

        // 2. Read entirely within a sparse extent
        let mut sparse_buf = vec![0xFFu8; 2000];
        let bytes_read = fs.read_file_range(&file_node, 5000, &mut sparse_buf).unwrap();
        assert_eq!(bytes_read, 2000);
        assert_eq!(&sparse_buf, &[0x00; 2000]);

        // 3. Read spanning Plain -> Sparse boundary
        let mut span_start_buf = vec![0xFFu8; 200];
        let bytes_read = fs.read_file_range(&file_node, 4000, &mut span_start_buf).unwrap();
        assert_eq!(bytes_read, 200);
        assert_eq!(&span_start_buf[..96], &[0xAA; 96]); // 4000..4096
        assert_eq!(&span_start_buf[96..], &[0x00; 104]); // 4096..4200

        // 4. Read spanning Sparse -> Plain boundary
        let mut span_end_buf = vec![0xFFu8; 200];
        let bytes_read = fs.read_file_range(&file_node, 12200, &mut span_end_buf).unwrap();
        assert_eq!(bytes_read, 200);
        assert_eq!(&span_end_buf[..88], &[0x00; 88]); // 12200..12288
        assert_eq!(&span_end_buf[88..], &[0xBB; 112]); // 12288..12400

        // 5. Read spanning multiple sparse clusters (Plain -> Sparse 1 -> Sparse 2 -> Plain)
        let mut multi_sparse_buf = vec![0xFFu8; 9000];
        let bytes_read = fs.read_file_range(&file_node, 4000, &mut multi_sparse_buf).unwrap();
        assert_eq!(bytes_read, 9000);
        assert_eq!(&multi_sparse_buf[..96], &[0xAA; 96]); // 4000..4096
        assert_eq!(&multi_sparse_buf[96..8288], &[0x00; 8192]); // 4096..12288
        assert_eq!(&multi_sparse_buf[8288..9000], &[0xBB; 712]); // 12288..13000
    }

    fn load_image(file: &str) -> Vec<u8> {
        fs::read(format!("/pkg/data/{file}")).expect("failed to read test file")
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[test_case("simple_lz4.erofs" ; "4096 block size lz4 compressed")]
    #[test_case("simple_lz4_legacy.erofs" ; "4096 block size lz4 legacy compressed")]
    #[fuchsia::test]
    fn test_parse_superblock(file: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles.clone()));
        // The fs validates the superblock during construction.
        let _fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");

        // Now mutate a byte in the superblock. This ensures the checksumming is actually happening
        // and getting evaluated correctly.
        let mut mutated_runfiles = runfiles.clone();
        mutated_runfiles[1088] ^= 0xFF;

        let reader = Arc::new(VecReader::new(mutated_runfiles));
        let fs = ErofsFilesystem::new(reader);
        assert!(fs.is_err());
        match fs.err().unwrap() {
            ErofsError::Parse(ParsingError::ChecksumMismatch(_, _)) => {}
            e => panic!("Expected ChecksumMismatch error, got {:?}", e),
        }
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[test_case("simple_lz4.erofs" ; "4096 block size lz4 compressed")]
    #[test_case("simple_lz4_legacy.erofs" ; "4096 block size lz4 legacy compressed")]
    #[fuchsia::test]
    fn test_list_dir(file: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        let mut buf = vec![DirectoryEntry::default(); 16];
        let filled = fs.read_directory(&root_node, 0, &mut buf).expect("failed to read directory");

        let names: Vec<String> = buf[..filled].iter().map(|e| e.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                ".",
                "..",
                "file1",
                "large_dir",
                "mixed_compression",
                "photosynthesis",
                "quantum",
                "symlink_to_file1",
            ]
        );
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[test_case("simple_lz4.erofs" ; "4096 block size lz4 compressed")]
    #[test_case("simple_lz4_legacy.erofs" ; "4096 block size lz4 legacy compressed")]
    #[fuchsia::test]
    fn test_overflow_nid(file: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let result = fs.node(u64::MAX);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ErofsError::Parse(ParsingError::InvalidNid(u64::MAX)));
    }

    #[test_case("simple.erofs", "file1" ; "4096 block size file1")]
    #[test_case("simple_512.erofs", "file1" ; "512 block size file1")]
    #[test_case("simple_lz4.erofs", "file1" ; "4096 block size lz4 file1")]
    #[test_case("simple_lz4_legacy.erofs", "file1" ; "4096 block size lz4 legacy file1")]
    #[test_case("simple.erofs", "photosynthesis" ; "4096 block size photosynthesis")]
    #[test_case("simple_512.erofs", "photosynthesis" ; "512 block size photosynthesis")]
    #[test_case("simple_lz4.erofs", "photosynthesis" ; "4096 block size lz4 photosynthesis")]
    #[test_case("simple_lz4_legacy.erofs", "photosynthesis" ; "4096 block size lz4 legacy photosynthesis")]
    #[test_case("simple_lz4.erofs", "quantum" ; "4096 block size lz4 quantum")]
    #[test_case("simple_lz4_legacy.erofs", "quantum" ; "4096 block size lz4 legacy quantum")]
    #[test_case("simple.erofs", "mixed_compression" ; "4096 block size mixed_compression")]
    #[test_case("simple_512.erofs", "mixed_compression" ; "512 block size mixed_compression")]
    #[test_case("simple_lz4.erofs", "mixed_compression" ; "4096 block size lz4 mixed_compression")]
    #[test_case("simple_lz4_legacy.erofs", "mixed_compression" ; "4096 block size lz4 legacy mixed_compression")]
    #[fuchsia::test]
    fn test_read_file_range(file: &str, name: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        let node = fs.lookup(&root_node, name).expect("failed to lookup").expect("file not found");
        let file_node = match node {
            Node::File(f) => f,
            _ => panic!("Expected file node"),
        };

        let size = file_node.size() as usize;
        let mut buf = vec![0u8; size];
        let bytes_read = fs.read_file_range(&file_node, 0, &mut buf).expect("failed to read");
        assert_eq!(bytes_read, size);
        let expected =
            fs::read(format!("/pkg/data/simple/{}", name)).expect("failed to read source file");
        assert_eq!(buf, expected);

        // Test partial read within file
        let mut buf = vec![0u8; 5];
        let bytes_read =
            fs.read_file_range(&file_node, 5, &mut buf).expect("failed to read partial");
        assert_eq!(bytes_read, 5);
        assert_eq!(&buf, &expected[5..10]);

        // Test non-extent-aligned seek reads into multi-cluster extents
        if size > 5000 {
            let mut buf = vec![0u8; 100];
            let bytes_read =
                fs.read_file_range(&file_node, 5000, &mut buf).expect("failed to read at 5000");
            assert_eq!(bytes_read, 100);
            assert_eq!(&buf, &expected[5000..5100]);
        }
        if size > 15000 {
            let mut buf = vec![0u8; 200];
            let bytes_read =
                fs.read_file_range(&file_node, 15000, &mut buf).expect("failed to read at 15000");
            assert_eq!(bytes_read, 200);
            assert_eq!(&buf, &expected[15000..15200]);
        }

        // Test read spanning across EOF (buffer larger than remaining data)
        let mut buf = vec![0u8; 100];
        let bytes_read = fs
            .read_file_range(&file_node, (size - 5) as u64, &mut buf)
            .expect("failed to read past eof");
        assert_eq!(bytes_read, 5);
        if name == "file1" {
            assert_eq!(&buf[..5], b"file\n");
        }

        // Test read at EOF
        let mut buf = vec![0u8; 100];
        let bytes_read =
            fs.read_file_range(&file_node, size as u64, &mut buf).expect("failed to read");
        assert_eq!(bytes_read, 0);
    }

    #[test_case("simple_lz4.erofs" ; "4096 block size lz4 compressed")]
    #[test_case("simple_lz4_legacy.erofs" ; "4096 block size lz4 legacy compressed")]
    #[fuchsia::test]
    fn test_mixed_compression(file: &str) {
        let runfiles = load_image(file);
        let fs = ErofsFilesystem::new(Arc::new(VecReader::new(runfiles))).unwrap();
        let root = fs.root_node();
        let node = fs.lookup(&root, "mixed_compression").unwrap().unwrap();
        let Node::File(file_node) = node else { panic!() };

        let lcluster_size = fs.block_size();
        let totalidx = file_node.total_lclusters(lcluster_size);
        let mut has_compressed_head = false;
        let mut has_plain_cluster = false;
        for lcn in 0..totalidx {
            if let Ok(LClusterEntry::Head(head)) = fs.read_lcluster_entry(&file_node.0, lcn) {
                if head.cluster_type.is_head() && head.cluster_type != LClusterType::Plain {
                    has_compressed_head = true;
                }
                if head.cluster_type == LClusterType::Plain {
                    has_plain_cluster = true;
                }
            }
        }
        assert!(
            has_compressed_head && has_plain_cluster,
            "mixed_compression should contain some plain clusters"
        );

        let size = file_node.size() as usize;
        let mut buf = vec![0u8; size];
        let bytes_read = fs.read_file_range(&file_node, 0, &mut buf).expect("failed to read");
        assert_eq!(bytes_read, size);

        let expected =
            fs::read("/pkg/data/simple/mixed_compression").expect("failed to read source file");
        assert_eq!(buf, expected);
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[test_case("simple_lz4.erofs" ; "4096 block size lz4 compressed")]
    #[test_case("simple_lz4_legacy.erofs" ; "4096 block size lz4 legacy compressed")]
    #[fuchsia::test]
    fn test_read_symlink(file: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        let node = fs
            .lookup(&root_node, "symlink_to_file1")
            .expect("failed to lookup")
            .expect("symlink not found");
        let symlink_node = match node {
            Node::Symlink(s) => s,
            _ => panic!("Expected symlink node"),
        };

        let target = fs.read_symlink(&symlink_node).expect("failed to read symlink");
        assert_eq!(target, b"file1");

        let selinux_val = fs.get_xattr(&symlink_node, b"security.selinux").unwrap().unwrap();
        assert_eq!(selinux_val, b"u:object_r:symlink_t:s0");
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[fuchsia::test]
    fn test_read_directory_pagination(file: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        let expected_names = vec![
            ".",
            "..",
            "file1",
            "large_dir",
            "mixed_compression",
            "photosynthesis",
            "quantum",
            "symlink_to_file1",
        ];

        // Test reading with buffer size 2 (pagination)
        let mut buf = vec![DirectoryEntry::default(); 2];

        // Page 1 (offset 0)
        let filled = fs.read_directory(&root_node, 0, &mut buf).expect("failed to read dir");
        assert_eq!(filled, 2);
        assert_eq!(buf[0].name, expected_names[0]);
        assert_eq!(buf[1].name, expected_names[1]);

        // Page 2 (offset 2)
        let filled = fs.read_directory(&root_node, 2, &mut buf).expect("failed to read dir");
        assert_eq!(filled, 2);
        assert_eq!(buf[0].name, expected_names[2]);
        assert_eq!(buf[1].name, expected_names[3]);

        // Page 3 (offset 4)
        let filled = fs.read_directory(&root_node, 4, &mut buf).expect("failed to read dir");
        assert_eq!(filled, 2);
        assert_eq!(buf[0].name, expected_names[4]);
        assert_eq!(buf[1].name, expected_names[5]);

        // Page 4 (offset 6)
        let filled = fs.read_directory(&root_node, 6, &mut buf).expect("failed to read dir");
        assert_eq!(filled, 2);
        assert_eq!(buf[0].name, expected_names[6]);
        assert_eq!(buf[1].name, expected_names[7]);

        // Page 5 (offset 8 - EOF)
        let filled = fs.read_directory(&root_node, 8, &mut buf).expect("failed to read dir");
        assert_eq!(filled, 0);

        // Test reading with buffer size 1 (extreme pagination)
        let mut buf1 = vec![DirectoryEntry::default(); 1];
        for i in 0..expected_names.len() {
            let filled = fs.read_directory(&root_node, i, &mut buf1).expect("failed to read dir");
            assert_eq!(filled, 1);
            assert_eq!(buf1[0].name, expected_names[i]);
        }
        let filled = fs
            .read_directory(&root_node, expected_names.len(), &mut buf1)
            .expect("failed to read dir");
        assert_eq!(filled, 0);
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[test_case("simple_lz4.erofs" ; "4096 block size lz4 compressed")]
    #[test_case("simple_lz4_legacy.erofs" ; "4096 block size lz4 legacy compressed")]
    #[fuchsia::test]
    fn test_read_directory_large_dir(file: &str) {
        // Note: the large directory in the golden image is only large enough to split the entries
        // into multiple blocks on the 512 block size golden.
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        let large_dir_node = fs
            .lookup(&root_node, "large_dir")
            .expect("failed to look up large_dir")
            .expect("large_dir not found");

        let large_dir = match large_dir_node {
            Node::Directory(d) => d,
            _ => panic!("Expected directory node"),
        };

        assert_eq!(fs.get_xattr(&root_node, b"security.selinux").unwrap(), None);
        let selinux_val = fs.get_xattr(&large_dir, b"security.selinux").unwrap().unwrap();
        assert_eq!(
            selinux_val,
            b"u:object_r:very_long_selinux_context_exceeding_the_inline_limit_of_two_hundred_and_fifty_six_bytes_and_requiring_the_use_of_extended_attributes_instead_of_returning_the_context_inline_in_the_node_attributes_table_representation_as_dictated_by_the_fuchsia_io_node_fidl_specification:s0"
        );
        let file1_node = fs.lookup(&large_dir, "file_number_1").unwrap().unwrap();
        let file1_selinux = fs.get_xattr(&file1_node, b"security.selinux").unwrap().unwrap();
        assert_eq!(
            file1_selinux,
            b"u:object_r:very_long_selinux_context_exceeding_the_inline_limit_of_two_hundred_and_fifty_six_bytes_and_requiring_the_use_of_extended_attributes_instead_of_returning_the_context_inline_in_the_node_attributes_table_representation_as_dictated_by_the_fuchsia_io_node_fidl_specification:s0"
        );

        // Skip the first two entries, . and ..
        let mut entry_offset = 2;
        let mut buffer = vec![DirectoryEntry::default(); 16];
        loop {
            let filled = fs.read_directory(&large_dir, entry_offset, &mut buffer).unwrap();
            for i in 0..filled {
                // check the prefix
                assert_eq!(buffer[i].name[..12], format!("file_number_"));
            }
            if filled < buffer.len() {
                break;
            }
            entry_offset += filled;
        }
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[test_case("simple_lz4.erofs" ; "4096 block size lz4 compressed")]
    #[test_case("simple_lz4_legacy.erofs" ; "4096 block size lz4 legacy compressed")]
    #[fuchsia::test]
    fn test_filesystem_metadata(file: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");

        assert!(fs.total_bytes() > 0);
        assert!(fs.total_inodes() > 0);
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[fuchsia::test]
    fn test_node_metadata(file: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        assert!(root_node.link_count() >= 2);
        assert!(root_node.mtime_ns() > 0);

        let file1_node = fs.lookup(&root_node, "file1").unwrap().unwrap();
        assert_eq!(file1_node.link_count(), 1);
        assert!(file1_node.mtime_ns() > 0);
    }

    #[test_case("simple.erofs" ; "4096 block size")]
    #[test_case("simple_512.erofs" ; "512 block size")]
    #[fuchsia::test]
    fn test_xattrs(file: &str) {
        let runfiles = load_image(file);
        let reader = Arc::new(VecReader::new(runfiles));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        // Check file1 (has both inline and shared xattrs)
        let file1_node = fs.lookup(&root_node, "file1").unwrap().unwrap();

        let xattr_names = fs.list_xattrs(&file1_node).unwrap();
        // Should contain user.flavor, user.security, user.shared, security.selinux
        assert!(xattr_names.contains(&b"user.flavor".to_vec()));
        assert!(xattr_names.contains(&b"user.security".to_vec()));
        assert!(xattr_names.contains(&b"user.shared".to_vec()));
        assert!(xattr_names.contains(&b"security.selinux".to_vec()));
        assert_eq!(xattr_names.len(), 4);

        let flavor_val = fs.get_xattr(&file1_node, b"user.flavor").unwrap().unwrap();
        assert_eq!(flavor_val, b"vanilla");

        let security_val = fs.get_xattr(&file1_node, b"user.security").unwrap().unwrap();
        assert_eq!(security_val, b"high");

        let shared_val = fs.get_xattr(&file1_node, b"user.shared").unwrap().unwrap();
        assert_eq!(shared_val, b"same_value");

        let selinux_val = fs.get_xattr(&file1_node, b"security.selinux").unwrap().unwrap();
        assert_eq!(selinux_val, b"u:object_r:file1_t:s0");

        // Check photosynthesis (has only shared xattr)
        let photo_node = fs.lookup(&root_node, "photosynthesis").unwrap().unwrap();

        let photo_xattrs = fs.list_xattrs(&photo_node).unwrap();
        assert_eq!(photo_xattrs, vec![b"user.shared".to_vec()]);

        let photo_shared_val = fs.get_xattr(&photo_node, b"user.shared").unwrap().unwrap();
        assert_eq!(photo_shared_val, b"same_value");
        assert_eq!(fs.get_xattr(&photo_node, b"security.selinux").unwrap(), None);

        // Check quantum (has no xattrs)
        let quantum_node = fs.lookup(&root_node, "quantum").unwrap().unwrap();
        assert_eq!(fs.get_xattr(&quantum_node, b"security.selinux").unwrap(), None);

        // Verify that we can still read the file content of photosynthesis
        let file_node = match photo_node {
            Node::File(f) => f,
            _ => panic!("Expected file node"),
        };
        let size = file_node.size() as usize;
        let mut buf = vec![0u8; size];
        let bytes_read = fs.read_file_range(&file_node, 0, &mut buf).expect("failed to read");
        assert_eq!(bytes_read, size);
        assert!(size > 0);

        // Check non-existent xattr
        let val = fs.get_xattr(&file1_node, b"user.non_existent").unwrap();
        assert_eq!(val, None);
    }

    // EROFS doesn't seem to make shared xattr groups with simple xattrs like the one above (I
    // assume it has some heuristics for judging when it is worth the cost) so this test manually
    // constructs a shared xattr area to test that part of the parsing logic.
    #[fuchsia::test]
    fn test_shared_xattr_parsing() {
        let block_size = 4096usize;
        let mut buf = vec![0u8; 3 * block_size];

        // Superblock at offset 1024
        let sb = format::SuperBlock {
            magic: LEU32::new(format::EROFS_MAGIC),
            checksum: LEU32::new(0),
            feature_compat: LEU32::new(0),
            block_size_bits: 12,
            sb_ext_slots: 0,
            root_nid: LEU16::new(0),
            inode_count: LEU64::new(1),
            epoch: LEU64::new(0),
            fixed_nsec: LEU32::new(0),
            blocks: LEU32::new(3),
            meta_block_addr: LEU32::new(1),
            xattr_block_addr: LEU32::new(0),
            uuid: [0; 16],
            volume_name: [0; 16],
            feature_incompat: LEU32::new(0),
            available_compr_algs: LEU16::new(0),
            extra_devices: LEU32::new(0),
            dirblkbits: 0,
            reserved: [0; 37],
        };
        buf[1024..1024 + 128].copy_from_slice(sb.as_bytes());

        // Compact Inode at meta_block_addr (block 1, offset 4096)
        let inode = format::InodeCompact {
            format: LEU16::new(0),
            xattr_icount: LEU16::new(2),
            mode: LEU16::new(0o040755),
            link_count: LEU16::new(1),
            size: LEU32::new(0),
            reserved_1: [0; 4],
            i_u: [0; 4],
            ino: LEU32::new(0),
            uid: LEU16::new(0),
            gid: LEU16::new(0),
            reserved_2: [0; 4],
        };
        buf[4096..4096 + 32].copy_from_slice(inode.as_bytes());

        // XattrInlineBodyHeader at offset 4128
        let header = format::XattrInlineBodyHeader {
            name_filter: LEU32::new(0),
            shared_count: 1,
            reserved: [0; 7],
        };
        buf[4128..4128 + 12].copy_from_slice(header.as_bytes());
        // shared_id index 512 (512 * 4 = offset 2048 in block 0) at offset 4140
        buf[4140..4144].copy_from_slice(&512u32.to_le_bytes());

        // Shared Xattr Entry at xattr_block_addr (block 0, offset 2048)
        let xentry = format::XattrEntry {
            name_len: 6,
            name_index: 1, // "user."
            value_size: LEU16::new(10),
        };
        buf[2048..2052].copy_from_slice(xentry.as_bytes());
        buf[2052..2058].copy_from_slice(b"shared");
        buf[2058..2068].copy_from_slice(b"same_value");

        let reader = Arc::new(VecReader::new(buf));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        let xattr_names = fs.list_xattrs(&root_node).expect("failed to list xattrs");
        assert_eq!(xattr_names, vec![b"user.shared".to_vec()]);

        let shared_val =
            fs.get_xattr(&root_node, b"user.shared").expect("failed to get xattr").unwrap();
        assert_eq!(shared_val, b"same_value");
    }

    #[fuchsia::test]
    fn test_xattr_iterator_fusing_on_error() {
        let block_size = 4096usize;
        let mut buf = vec![0u8; 3 * block_size];

        let sb = format::SuperBlock {
            magic: LEU32::new(format::EROFS_MAGIC),
            checksum: LEU32::new(0),
            feature_compat: LEU32::new(0),
            block_size_bits: 12,
            sb_ext_slots: 0,
            root_nid: LEU16::new(0),
            inode_count: LEU64::new(1),
            epoch: LEU64::new(0),
            fixed_nsec: LEU32::new(0),
            blocks: LEU32::new(3),
            meta_block_addr: LEU32::new(1),
            xattr_block_addr: LEU32::new(2),
            uuid: [0; 16],
            volume_name: [0; 16],
            feature_incompat: LEU32::new(0),
            available_compr_algs: LEU16::new(0),
            extra_devices: LEU32::new(0),
            dirblkbits: 0,
            reserved: [0; 37],
        };
        buf[1024..1024 + 128].copy_from_slice(sb.as_bytes());

        let inode = format::InodeCompact {
            format: LEU16::new(0),
            xattr_icount: LEU16::new(3),
            mode: LEU16::new(0o040755),
            link_count: LEU16::new(1),
            size: LEU32::new(0),
            reserved_1: [0; 4],
            i_u: [0; 4],
            ino: LEU32::new(0),
            uid: LEU16::new(0),
            gid: LEU16::new(0),
            reserved_2: [0; 4],
        };
        buf[4096..4096 + 32].copy_from_slice(inode.as_bytes());

        let header = format::XattrInlineBodyHeader {
            name_filter: LEU32::new(0),
            shared_count: 2,
            reserved: [0; 7],
        };
        buf[4128..4128 + 12].copy_from_slice(header.as_bytes());
        buf[4140..4144].copy_from_slice(&0u32.to_le_bytes());
        buf[4144..4148].copy_from_slice(&1u32.to_le_bytes());

        // Invalid shared xattr entry (invalid name_index 99) at xattr_block_addr for shared_id 0
        // (offset 8192)
        let invalid_xentry =
            format::XattrEntry { name_len: 6, name_index: 99, value_size: LEU16::new(10) };
        buf[8192..8196].copy_from_slice(invalid_xentry.as_bytes());

        let reader = Arc::new(VecReader::new(buf));
        let fs = ErofsFilesystem::new(reader).expect("failed to parse superblock");
        let root_node = fs.root_node();

        let mut iter = fs.iter_xattrs(&root_node).expect("failed to create iterator");
        assert!(iter.next().unwrap().is_err());
        // Iterator MUST be fused now: subsequent next() calls MUST return None
        assert!(iter.next().is_none());
    }
}
