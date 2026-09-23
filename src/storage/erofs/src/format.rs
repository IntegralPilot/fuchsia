// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Raw on-disk format structs for erofs. See
//! https://erofs.docs.kernel.org/en/latest/ondisk/core_ondisk.html for more details.

use static_assertions::assert_eq_size;
use zerocopy::byteorder::little_endian::{U16 as LEU16, U32 as LEU32, U64 as LEU64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// Magic number for erofs filesystems.
pub const EROFS_MAGIC: u32 = 0xE0F5E1E2;
pub const SUPERBLOCK_OFFSET: u64 = 1024;
pub const INODE_SLOT_SIZE: u64 = 32;
pub const DIRENT_SIZE: usize = std::mem::size_of::<Dirent>();

/// The on-disk format of an erofs superblock.
#[derive(Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct SuperBlock {
    /// Magic number. Should be equal to EROFS_MAGIC.
    pub magic: LEU32,
    /// CRC-32 checksum of the block containing the superblock. This field is set to zero in the
    /// checksummed version.
    pub checksum: LEU32,
    /// Feature flags. If any flags in here are not recognized the filesystem can still mount
    /// without a loss of correctness.
    pub feature_compat: LEU32,
    /// Block size stored as a power of two.
    pub block_size_bits: u8,
    /// Number of 16-byte superblock extension slots.
    pub sb_ext_slots: u8,
    /// Node ID of the root directory.
    pub root_nid: LEU16,
    /// Total number of inodes - primarily for statfs. Can potentially be set to zero, don't rely
    /// on it for validation.
    pub inode_count: LEU64,
    /// UNIX timestamp of when the filesystem was created, used as mtime for compact inodes.
    pub epoch: LEU64,
    /// Fixed nanosecond timestamp used as mtime for compact inodes.
    pub fixed_nsec: LEU32,
    /// Total number of blocks - primarily for statfs. Can potentially be set to zero, don't rely
    /// on it for validation.
    pub blocks: LEU32,
    /// Start block address of inode metadata zone. This is essentially an offset for node id
    /// calculations, not a guarantee that the inode data actually starts at this offset.
    pub meta_block_addr: LEU32,
    /// Start block address of the xattr zone. Similar to meta_block_addr.
    pub xattr_block_addr: LEU32,
    /// 128-bit UUID for this volume.
    pub uuid: [u8; 16],
    /// The volume name, zero-padded.
    pub volume_name: [u8; 16],
    /// Feature flags. If any flags here are not recognized, the filesystem can _not_ be mounted.
    pub feature_incompat: LEU32,
    /// Info about compression algorithms. Set to zero if image is not compressed. Otherwise it is
    /// either an indication of what the available compression algorithms are in a bitmap (if
    /// COMPR_CFGS is set in the incompat flags), or the lz4_max_distance.
    pub available_compr_algs: LEU16,
    /// External device support, ignored in core format.
    pub extra_devices: LEU32,
    /// Set to zero in the core format. This can be used to make directory blocks larger than
    /// regular blocks (it modifies the block_size_bits field).
    pub dirblkbits: u8,
    /// There are some other fields we will care about when we implement xattr and compression
    /// support, but for now with the core format we don't care about the rest of the superblock.
    // TODO(https://fxbug.dev/479841115): Implement xattr support.
    // TODO(https://fxbug.dev/479841115): Implement compression support.
    pub reserved: [u8; 37],
}
assert_eq_size!(SuperBlock, [u8; 128]);

/// Compact inode on-disk format. Fits within a single inode slot.
#[derive(Debug, Clone, Copy, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct InodeCompact {
    /// Format information about this particular inode. Indicates if it is compact or extended, and
    /// the data layout (i.e. what i_u means for this node).
    pub format: LEU16,
    /// Size of the inline xattr region, or zero if there are no xattrs. This is not a count of
    /// xattrs, it is plugged into a formula that determines the size in bytes of the xattr
    /// section.
    pub xattr_icount: LEU16,
    /// Standard unix file type and permission bits.
    pub mode: LEU16,
    /// Number of hard links.
    pub link_count: LEU16,
    /// File size in bytes.
    pub size: LEU32,
    /// Reserved section.
    pub reserved_1: [u8; 4],
    /// Inode data union - the exact meaning of this field is dependent on the inode format field.
    /// For uncompressed data layouts (FlatPlain, FlatInline), this is the raw block address. For
    /// compressed data layouts (CompressedFull, CompressedCompact), this is the compressed block
    /// count.
    pub i_u: [u8; 4],
    /// Inode number for stat compatibility.
    pub ino: LEU32,
    /// Owner UID.
    pub uid: LEU16,
    /// Owner GID.
    pub gid: LEU16,
    /// Reserved section.
    pub reserved_2: [u8; 4],
}
assert_eq_size!(InodeCompact, [u8; 32]);

/// Extended inode on-disk format. Uses two inode slots. Allows for more metadata and also larger
/// files.
#[derive(Debug, Clone, Copy, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct InodeExtended {
    /// Format information about this particular inode. Indicates if it is compact or extended, and
    /// the data layout (i.e. what i_u means for this node).
    pub format: LEU16,
    /// Size of the inline xattr region, or zero if there are no xattrs. This is not a count of
    /// xattrs, it is plugged into a formula that determines the size in bytes of the xattr
    /// section.
    pub xattr_icount: LEU16,
    /// Standard unix file type and permission bits.
    pub mode: LEU16,
    /// Reserved section.
    pub reserved_1: [u8; 2],
    /// File size in bytes.
    pub size: LEU64,
    /// Inode data union - the exact meaning of this field is dependent on the inode format field.
    /// For uncompressed data layouts (FlatPlain, FlatInline), this is the raw block address. For
    /// compressed data layouts (CompressedFull, CompressedCompact), this is the compressed block
    /// count.
    pub i_u: [u8; 4],
    /// Inode number for stat compatibility.
    pub ino: LEU32,
    /// Owner UID.
    pub uid: LEU32,
    /// Owner GID.
    pub gid: LEU32,
    /// Last modification time in seconds since unix epoch.
    pub mtime: LEU64,
    /// Nanosecond part of the last modification time.
    pub mtime_ns: LEU32,
    /// Number of hard links.
    pub link_count: LEU32,
    /// Reserved section.
    pub reserved_2: [u8; 16],
}
assert_eq_size!(InodeExtended, [u8; 64]);

/// Directory entry on-disk format. Contained within the directory data blocks.
#[derive(Debug, Clone, Copy, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct Dirent {
    /// Node number of the target inode for this entry.
    pub nid: LEU64,
    /// Byte offset of the filename, relative to the start of this block. The name offset of the
    /// first dirent in a directory block indicates how many entries there are in that block.
    pub nameoff: LEU16,
    /// File type code.
    pub file_type: u8,
    /// Reserved section.
    pub reserved: u8,
}
assert_eq_size!(Dirent, [u8; 12]);

/// Inlined xattr body header. This sits immediately following the inode metadata if the
/// xattr_icount is non-zero, providing information about the structure of the following xattr
/// entries.
#[derive(Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct XattrInlineBodyHeader {
    /// If EROFS_FEATURE_COMPAT_XATTR_FILTER is enabled and supported, this is an inverted bloom
    /// filter on the xattr key, which can be used to determine if a key is either definitely
    /// absent (fast failure without reading/parsing) or may exist.
    pub name_filter: LEU32,
    /// The number of 4 byte entries immediately after this header that are 32-bit indexes into the
    /// global shared xattr pool, located at the xattr_block_addr from the superblock. These shared
    /// xattrs dedupe identical key-value pairs across inodes to save space.
    pub shared_count: u8,
    /// Reserved section. Must be zero.
    pub reserved: [u8; 7],
}
assert_eq_size!(XattrInlineBodyHeader, [u8; 12]);

/// Xattr entry record header. Describes an individual xattr key-value pair for this inode. The
/// name suffix and value data immediately follow this header, padded to a 4-byte boundary.
#[derive(Debug, Clone, Copy, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct XattrEntry {
    /// Length in bytes of the name suffix (i.e. this length does not include the prefix section).
    /// The data is stored immediately following this entry.
    pub name_len: u8,
    /// Index of the attribute's namespace prefix. There are 5 built-in prefixes -
    ///  - 1 -> "user."
    ///  - 2 -> "system.posix_acl_access"
    ///  - 3 -> "system.posix_acl_default"
    ///  - 4 -> "trusted."
    ///  - 6 -> "security."
    /// If EROFS_FEATURE_INCOMPAT_XATTR_PREFIXES is set, this value is interpreted differently, but
    /// we don't support that right now.
    pub name_index: u8,
    /// Length in bytes of the value that follows the name suffix after this header.
    pub value_size: LEU16,
}
assert_eq_size!(XattrEntry, [u8; 4]);

/// Size of the compression map header area for legacy compressed inodes (8-byte header + 8-byte
/// reserved gap). This is for historical reasons.
pub const LEGACY_MAP_HEADER_SIZE: u64 = 16;

/// Compression metadata header. For inodes with compressed data layouts, this header is written
/// after the core metadata and extended attributes, padded to the next 8-byte boundary. It
/// contains the compression info for this particular inode.
#[derive(Debug, Clone, Copy, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct CompressionMapHeader {
    /// Reserved 2 byte section. There are incompat features that change the interpretation of the
    /// first four bytes of this header but we don't implement them, so this is always ignored
    /// today.
    pub reserved_1: [u8; 2],
    /// If the advisory flags indicate there is an inline pcluster, this indicates the size of the
    /// inline data in bytes. This is gated behind an incompat flag that we don't implement so it
    /// is not used in practice.
    pub inline_data_size: LEU16,
    /// Advisory flags for decompression.
    pub advisory_flags: LEU16,
    /// Compression algorithm types for logical clusters. We only support lz4 today so this should
    /// always be zero.
    pub algorithm_type: u8,
    /// The first 4 bits of this are a modifier to the block size bits in the superblock. This
    /// allows files to have logical cluster sizes that are larger than the block size. The default
    /// is to match block size. We ignore this value right now and only default to block size.
    pub lcluster_bits: u8,
}
assert_eq_size!(CompressionMapHeader, [u8; 8]);

/// Logical cluster index entry, for the legacy CompressedFull data layout. One of these exists per
/// logical cluster in the inode data.
#[derive(Debug, Clone, Copy, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct LClusterIndex {
    /// Advisory bits (including cluster type).
    pub advisory_flags: LEU16,
    /// If this logical cluster is a HEAD cluster, this holds the offset into the cluster where the
    /// data actually starts. The variable-length extents that map to physical clusters are not
    /// aligned in any meaningful way and can start in the middle of a logical cluster.
    pub extent_start_offset: LEU16,
    /// Depending on if this logical cluster is a HEAD/PLAIN or NONHEAD type, this has two
    /// different interpretations -
    ///  - For HEAD and PLAIN types, this is the 4-byte block address for the physical cluster.
    ///  - For NONHEAD, this is two 2-byte values. [0] is the distance back to its HEAD lcluster,
    ///    and [1] is the distance forward to the next HEAD lcluster.
    pub data_union: [u8; 4],
}
assert_eq_size!(LClusterIndex, [u8; 8]);
