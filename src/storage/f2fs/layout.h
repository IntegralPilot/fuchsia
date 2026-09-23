// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_LAYOUT_H_
#define SRC_STORAGE_F2FS_LAYOUT_H_

#include "src/storage/f2fs/common.h"

namespace f2fs {

constexpr uint64_t kSuperOffset = 1024;     // byte-size offset
constexpr uint32_t kMinLogSectorSize = 9;   // 9 bits for 512 byte
constexpr uint32_t kMaxLogSectorSize = 12;  // 12 bits for 4096 byte
constexpr int kMaxExtension = 64;           // # of extension entries

// Superblock location.
constexpr size_t kSuperblockStart = 0;
constexpr size_t kSuperblockCopies = 2;

// for mkfs
constexpr uint16_t kMajorVersion = 1;
constexpr uint16_t kMinorVersion = 0;

constexpr uint64_t kODirectory = 0x00004000;
constexpr uint64_t kOEonly = 0x00000040;
constexpr uint64_t kOWronly = 0x00000080;
constexpr uint64_t kORdonly = 0x00000100;

constexpr uint32_t kNumberOfCheckpointPack = 2;

constexpr uint32_t kDefaultSectorSize = 512;
constexpr uint32_t kDefaultSectorsPerBlock = 8;
constexpr uint32_t kDefaultLogBlocksPerSegment = 9;
constexpr uint32_t kDefaultBlocksPerSegment = 1 << kDefaultLogBlocksPerSegment;
constexpr uint32_t kDefaultSegmentsPerSection = 1;
constexpr uint32_t kDefaultSegmentSize =
    kDefaultSectorsPerBlock * kDefaultSectorSize * kDefaultBlocksPerSegment;
constexpr uint32_t kCpBlockSize = (kDefaultSectorSize * kDefaultSectorsPerBlock);
constexpr uint32_t kVolumeLabelLength = 16;
// At least, it requires six reserved sections to preallocate the next sections of all levels of
// temperature as well as two reserved sections to which the valid blocks of a gc victim migrate.
constexpr uint32_t kMinReservedSectionsForGc = 8;
// It requires at least 8 segments (sb + ssa + (ckpt + sit + nat) * 2) for its metadata
constexpr uint32_t kMinMetaSegments = 8;
constexpr uint32_t kDefaultOpRatio = 4;

// For further optimization on multi-head logs, on-disk layout supports maximum
// 16 logs by default. The number, 16, is expected to cover all the cases
// enoughly. The implementaion currently uses no more than 6 logs.
// Half the logs are used for nodes, and the other half are used for data.
constexpr int kMaxActiveLogs = 16;
constexpr int kMaxActiveNodeLogs = 8;
constexpr int kMaxActiveDataLogs = 8;

// By default, there are 6 active log areas across the whole main area.
// When considering hot and cold data separation to reduce cleaning overhead,
// we split 3 for data logs and 3 for node logs as hot, warm, and cold types,
// respectively.
// In the current design, you should not change the numbers intentionally.
// Instead, as a mount option such as active_logs=x, you can use 2, 4, and 6
// logs individually according to the underlying devices. (default: 6)
// Just in case, on-disk layout covers maximum 16 logs that consist of 8 for
// data and 8 for node logs.
constexpr int kNrCursegDataType = 3;
constexpr int kNrCursegNodeType = 3;
constexpr int kNrCursegType = kNrCursegDataType + kNrCursegNodeType;

enum class CursegType {
  kCursegHotData = 0,  // directory entry blocks
  kCursegWarmData,     // data blocks
  kCursegColdData,     // multimedia or GCed data blocks
  kCursegHotNode,      // direct node blocks of directory files
  kCursegWarmNode,     // direct node blocks of normal files
  kCursegColdNode,     // indirect node blocks
  kNoCheckType
};

// In VictimSelPolicy, there are two block allocation types.
// LFS writes data sequentially with cleaning operations.
// SSR (Slack Space Recycle) reuses obsolete space without cleaning operations.
enum class AllocType : uint8_t { kLFS = 0, kSSR };

constexpr uint32_t kMinVolumeSegments =
    kMinMetaSegments + kMinReservedSectionsForGc + kNrCursegType;

struct Superblock {
  uint32_t magic = 0;                  // Magic Number
  uint16_t major_ver = 0;              // Major Version
  uint16_t minor_ver = 0;              // Minor Version
  uint32_t log_sectorsize = 0;         // log2 sector size in bytes
  uint32_t log_sectors_per_block = 0;  // log2 # of sectors per block
  uint32_t log_blocksize = 0;          // log2 block size in bytes
  uint32_t log_blocks_per_seg = 0;     // log2 # of blocks per segment
  uint32_t segs_per_sec = 0;           // # of segments per section
  uint32_t secs_per_zone = 0;          // # of sections per zone
  uint32_t checksum_offset = 0;        // checksum offset inside super block
  uint64_t block_count = 0;            // total # of user blocks
  uint32_t section_count = 0;          // total # of sections
  uint32_t segment_count = 0;          // total # of segments
  uint32_t segment_count_ckpt = 0;     // # of segments for checkpoint
  uint32_t segment_count_sit = 0;      // # of segments for SIT
  uint32_t segment_count_nat = 0;      // # of segments for NAT
  uint32_t segment_count_ssa = 0;      // # of segments for SSA
  uint32_t segment_count_main = 0;     // # of segments for main area
  uint32_t segment0_blkaddr = 0;       // start block address of segment 0
  uint32_t cp_blkaddr = 0;             // start block address of checkpoint
  uint32_t sit_blkaddr = 0;            // start block address of SIT
  uint32_t nat_blkaddr = 0;            // start block address of NAT
  uint32_t ssa_blkaddr = 0;            // start block address of SSA
  uint32_t main_blkaddr = 0;           // start block address of main area
  uint32_t root_ino = 0;               // root inode number
  uint32_t node_ino = 0;               // node inode number
  uint32_t meta_ino = 0;               // meta inode number
  uint8_t uuid[16] = {
      0,
  };  // 128-bit uuid for volume
  uint16_t volume_name[512] = {};                 // volume name
  uint32_t extension_count = 0;                   // # of extensions below
  uint8_t extension_list[kMaxExtension][8] = {};  // extension array
  uint32_t cp_payload = 0;                        // # of checkpoint trailing blocks for SIT bitmap
} __attribute__((packed));

// For checkpoint
enum class CpFlag {
  kCpCrcRecoveryFlag = 0x40,
  kCpErrorFlag = 0x8,
  kCpCompactSumFlag = 0x4,
  kCpOrphanPresentFlag = 0x2,
  kCpUmountFlag = 0x1,
};

struct Checkpoint {
  uint64_t checkpoint_ver = 0;          // checkpoint block version number
  uint64_t user_block_count = 0;        // # of user blocks
  uint64_t valid_block_count = 0;       // # of valid blocks in main area
  uint32_t rsvd_segment_count = 0;      // # of reserved segments for gc
  uint32_t overprov_segment_count = 0;  // # of overprovision segments
  uint32_t free_segment_count = 0;      // # of free segments in main area

  // information of current node segments
  uint32_t cur_node_segno[kMaxActiveNodeLogs];
  uint16_t cur_node_blkoff[kMaxActiveNodeLogs];
  // information of current data segments
  uint32_t cur_data_segno[kMaxActiveDataLogs];
  uint16_t cur_data_blkoff[kMaxActiveDataLogs];
  uint32_t ckpt_flags = 0;                 // Flags : umount and journal_present
  uint32_t cp_pack_total_block_count = 0;  // total # of one cp pack
  uint32_t cp_pack_start_sum = 0;          // start block number of data summary
  uint32_t valid_node_count = 0;           // Total number of valid nodes
  uint32_t valid_inode_count = 0;          // Total number of valid inodes
  uint32_t next_free_nid = 0;              // Next free node number
  uint32_t sit_ver_bitmap_bytesize = 0;    // Default value 64
  uint32_t nat_ver_bitmap_bytesize = 0;    // Default value 256
  uint32_t checksum_offset = 0;            // checksum offset inside cp block
  uint64_t elapsed_time = 0;               // mounted time
  // allocation type of current segment
  uint8_t alloc_type[kMaxActiveLogs];

  // SIT/NAT version bitmap
  uint8_t sit_nat_version_bitmap[1];
  static constexpr uint32_t GetHeaderByteSize() { return sizeof(Checkpoint) - 1; }
} __attribute__((packed));

constexpr uint32_t kChecksumOffset = kBlockSize - sizeof(uint32_t);
constexpr size_t kMaxBitmapBytes = kChecksumOffset - Checkpoint::GetHeaderByteSize();

inline uint64_t VersionBitmapByteSize(uint32_t segment_count, uint32_t log_blocks_per_seg) {
  return (uint64_t{segment_count} / 2 << log_blocks_per_seg) / 8;
}

// For orphan inode management
constexpr uint16_t kOrphansPerBlock = 1020;

struct OrphanBlock {
  uint32_t ino[kOrphansPerBlock];  // inode numbers
  uint32_t reserved = 0;           // reserved
  uint16_t blk_addr = 0;           // block index in current CP
  uint16_t blk_count = 0;          // Number of orphan inode blocks in CP
  uint32_t entry_count = 0;        // Total number of orphan nodes in current CP
  uint32_t check_sum = 0;          // CRC32 for orphan inode block
} __attribute__((packed));

// For NODE structure
struct Extent {
  uint32_t fofs = 0;      // start file offset of the extent
  uint32_t blk_addr = 0;  // start block address of the extent
  uint32_t len = 0;       // lengh of the extent
} __attribute__((packed));

constexpr uint32_t kMaxNameLen = NAME_MAX;
constexpr int kAddrsPerInode = 923;   // Address Pointers in an Inode
constexpr int kNidsPerInode = 5;      // Node IDs in an Inode
constexpr int kAddrsPerBlock = 1018;  // Address Pointers in a Direct Block
constexpr int kNidsPerBlock = 1018;   // Node IDs in an Indirect Block

constexpr uint32_t kDentrySlotLen = 8;  // One directory entry slot covers 8bytes-long file name

constexpr uint8_t kInlineStartOffset = 1;  // start offset of inline dentries
constexpr uint8_t kInlineXattrAddrs = 50;  // 200 bytes for inline xattrs
constexpr uint8_t kInlineXattr = 0x01;     // file inline xattr flag
constexpr uint8_t kInlineData = 0x02;      // file inline data flag
constexpr uint8_t kInlineDentry = 0x04;    // file inline dentry flag
constexpr uint8_t kDataExist = 0x08;       // file inline data exist flag
constexpr uint8_t kExtraAttr = 0x20;       // file having extra attribute

struct Inode {
  uint16_t i_mode = 0;           // file mode
  uint8_t i_advise = 0;          // file hints
  uint8_t i_inline = 0;          // file inline flags
  uint32_t i_uid = 0;            // user ID
  uint32_t i_gid = 0;            // group ID
  uint32_t i_links = 0;          // links count
  uint64_t i_size = 0;           // file size in bytes
  uint64_t i_blocks = 0;         // file size in blocks
  uint64_t i_atime = 0;          // access time
  uint64_t i_ctime = 0;          // change time
  uint64_t i_mtime = 0;          // modification time
  uint32_t i_atime_nsec = 0;     // access time in nano scale
  uint32_t i_ctime_nsec = 0;     // change time in nano scale
  uint32_t i_mtime_nsec = 0;     // modification time in nano scale
  uint32_t i_generation = 0;     // file version (for NFS)
  uint32_t i_current_depth = 0;  // only for directory depth
  uint32_t i_xattr_nid = 0;      // nid to save xattr
  uint32_t i_flags = 0;          // file attributes
  uint32_t i_pino = 0;           // parent inode number
  uint32_t i_namelen = 0;        // file name length
  uint8_t i_name[kMaxNameLen];   // file name for SPOR
  uint8_t i_dir_level = 0;       // dentry_level for large dir

  Extent i_ext;  // caching a largest extent

  union {
    struct {
      uint16_t i_extra_isize;        // extra inode attribute size in bytes
      uint16_t i_inline_xattr_size;  // inline xattr size
    };
    uint32_t i_addr[kAddrsPerInode];  // Pointers to data blocks
  };

  uint32_t i_nid[kNidsPerInode];  // direct(2), indirect(2), double_indirect(1) node id
} __attribute__((packed));

// Span of the extra attribute fields Linux f2fs defines, from i_extra_isize through
// i_extra_end: i_extra_isize(2) + i_inline_xattr_size(2) + i_projid(4) + i_inode_checksum(4)
// + i_crtime(8) + i_crtime_nsec(4) + i_compr_blocks(8) + i_compress_algorithm(1) +
// i_log_cluster_size(1) + i_compress_flag(2). Keep in sync with upstream
// F2FS_TOTAL_EXTRA_ATTR_SIZE, which grows whenever a field is added there.
constexpr uint16_t kMaxExtraAttrSize = 36;
constexpr uint16_t kMinExtraAttrSize = sizeof(uint32_t);

// |i_extra_isize| shifts the start of the block address array an inode carries and
// |i_inline_xattr_size| shrinks its end, so together they decide how many entries
// VnodeF2fs::GetAddrsPerInode() reports. This mirrors the checks Linux f2fs runs on both
// fields when it loads an inode, before any later code trusts them.
inline bool IsValidInlineLayout(const Inode &inode) {
  if (!(inode.i_inline & kExtraAttr)) {
    return true;
  }
  const uint16_t extra_isize = LeToCpu(inode.i_extra_isize);
  if (extra_isize < kMinExtraAttrSize || extra_isize > kMaxExtraAttrSize ||
      extra_isize % sizeof(uint32_t)) {
    FX_LOGS(WARNING) << "inode reserves " << extra_isize
                     << " bytes for its extra attributes, outside [" << kMinExtraAttrSize << ", "
                     << kMaxExtraAttrSize << "] or not a multiple of " << sizeof(uint32_t);
    return false;
  }
  // Linux lets an image choose this size (its flexible inline xattr feature); this
  // implementation always reserves kInlineXattrAddrs entries, so any other size describes a
  // layout it cannot honor.
  if (inode.i_inline & kInlineXattr) {
    const uint16_t inline_xattr_size = LeToCpu(inode.i_inline_xattr_size);
    if (inline_xattr_size != kInlineXattrAddrs) {
      FX_LOGS(WARNING) << "inode reserves " << inline_xattr_size
                       << " address entries for its inline xattr, and this implementation "
                          "reserves "
                       << kInlineXattrAddrs;
      return false;
    }
  }
  return true;
}

struct DirectNode {
  uint32_t addr[kAddrsPerBlock];  // aray of data block address
} __attribute__((packed));

struct IndirectNode {
  uint32_t nid[kNidsPerBlock];  // aray of data block address
} __attribute__((packed));

enum class BitShift { kColdBitShift = 0, kFsyncBitShift, kDentBitShift, kOffsetBitShift };

struct NodeFooter {
  uint32_t nid = 0;           // node id
  uint32_t ino = 0;           // inode number
  uint32_t flag = 0;          // include cold/fsync/dentry marks and offset
  uint64_t cp_ver = 0;        // checkpoint version
  uint32_t next_blkaddr = 0;  // next node page block address
} __attribute__((packed));

struct Node {
  // can be one of three types: inode, direct, and indirect types
  union {
    Inode i;
    DirectNode dn;
    IndirectNode in;
  };
  NodeFooter footer;
} __attribute__((packed));

// For NAT entries
struct RawNatEntry {
  uint8_t version = 0;      // latest version of cached nat entry
  uint32_t ino = 0;         // inode number
  uint32_t block_addr = 0;  // block address
} __attribute__((packed));

constexpr uint32_t kNatEntryPerBlock = kBlockSize / sizeof(RawNatEntry);

struct NatBlock {
  RawNatEntry entries[kNatEntryPerBlock];
} __attribute__((packed));

// For SIT entries
//
// Each segment is 2MB in size by default so that a bitmap for validity of
// there-in blocks should occupy 64 bytes, 512 bits.
// Not allow to change this.
constexpr uint32_t kSitVBlockMapSize = 64;
constexpr uint32_t kSitVBlockMapSizeInBit = kSitVBlockMapSize << kShiftForBitSize;

// Note that SitEntry->vblocks has the following bit-field information.
// [15:10] : allocation type such as CURSEG_XXXX_TYPE
// [9:0] : valid block count
constexpr uint16_t kSitVblocksShift = 10;
constexpr uint16_t kSitVblocksMask = (1 << kSitVblocksShift) - 1;

// Note that SitEntry->vblocks has the following bit-field information.
// [15:10] : allocation type such as CURSEG_XXXX_TYPE
// [9:0] : valid block count
constexpr uint16_t kCurSegNull = 0x003f;  // use 6bit - 0x3f

struct SitEntry {
  uint16_t vblocks = 0;                  // reference above
  uint8_t valid_map[kSitVBlockMapSize];  // bitmap for valid blocks
  uint64_t mtime = 0;                    // segment age for cleaning
} __attribute__((packed));

constexpr uint32_t kSitEntryPerBlock = kBlockSize / sizeof(SitEntry);
constexpr uint32_t kMaxSitBitmapSize =
    (safemath::CheckLsh<uint32_t>(1, (32 - kDefaultLogBlocksPerSegment)) / kSitEntryPerBlock /
     kBitsPerByte)
        .ValueOrDie();
constexpr uint32_t kMaxCpPayload = (kMaxSitBitmapSize + kBlockSize - 1) / kBlockSize;
static_assert(kMaxCpPayload == 5);

inline uint16_t GetSitVblocks(const SitEntry &raw_sit) {
  return LeToCpu(raw_sit.vblocks) & kSitVblocksMask;
}
inline uint8_t GetSitType(const SitEntry &raw_sit) {
  return (LeToCpu(raw_sit.vblocks) & ~kSitVblocksMask) >> kSitVblocksShift;
}

struct SitBlock {
  SitEntry entries[kSitEntryPerBlock];
} __attribute__((packed));

// For segment summary
//
// One summary block contains exactly 512 summary entries, which represents
// exactly 2MB segment by default. Not allow to change the basic units.
//
// NOTE : For initializing fields, you must use set_summary
//
// - If data page, nid represents dnode's nid
// - If node page, nid represents the node page's nid.
//
// The ofs_in_node is used by only data page. It represents offset
// from node's page's beginning to get a data block address.
// ex) data_blkaddr = (block_t)(nodepage_start_address + ofs_in_node)

// a summary entry for a 4KB-sized block in a segment
struct Summary {
  uint32_t nid = 0;  // parent node id
  union {
    uint8_t reserved[3];
    struct {
      uint8_t version;       // node version number
      uint16_t ofs_in_node;  // block index in parent node
    } __attribute__((packed));
  };
} __attribute__((packed));

constexpr uint32_t kEntriesInSum = 512;
constexpr uint32_t kSummarySize = sizeof(Summary);
constexpr uint32_t kSumEntrySize = kSummarySize * kEntriesInSum;

// summary block type, node or data, is stored to the SummaryFooter
constexpr uint8_t kSumTypeNode = 1;
constexpr uint8_t kSumTypeData = 0;

struct SummaryFooter {
  uint8_t entry_type = 0;  // SUM_TYPE_XXX
  uint32_t check_sum = 0;  // summary checksum
} __attribute__((packed));

constexpr uint32_t kSumFooterSize = sizeof(SummaryFooter);
constexpr size_t kSumJournalSize = kBlockSize - kSumFooterSize - kSumEntrySize;

inline uint8_t GetSumType(SummaryFooter *footer) { return footer->entry_type; }
inline void SetSumType(SummaryFooter *footer, uint8_t type) { footer->entry_type = type; }

// frequently updated NAT/SIT entries can be stored in the spare area in
// summary blocks
enum class JournalType { kNatJournal = 0, kSitJournal };

struct NatJournalEntry {
  uint32_t nid = 0;
  RawNatEntry ne;
} __attribute__((packed));

constexpr size_t kNatJournalEntries = (kSumJournalSize - 2) / sizeof(NatJournalEntry);
constexpr size_t kNatJournalReserved = (kSumJournalSize - 2) % sizeof(NatJournalEntry);
struct NatJournal {
  NatJournalEntry entries[kNatJournalEntries];
  uint8_t reserved[kNatJournalReserved];
} __attribute__((packed));

struct SitJournalEntry {
  uint32_t segno = 0;
  SitEntry se;
} __attribute__((packed));

constexpr size_t kSitJournalEntries = (kSumJournalSize - 2) / sizeof(SitJournalEntry);
constexpr size_t kSitJournalReserved = (kSumJournalSize - 2) % sizeof(SitJournalEntry);

struct SitJournal {
  SitJournalEntry entries[kSitJournalEntries];
  uint8_t reserved[kSitJournalReserved];
} __attribute__((packed));

// 4KB-sized summary block structure
struct SummaryBlock {
  Summary entries[kEntriesInSum];
  union {
    uint16_t n_nats;
    uint16_t n_sits;
  };
  // spare area is used by NAT or SIT journals
  union {
    NatJournal nat_j;
    SitJournal sit_j;
  };
  SummaryFooter footer;
} __attribute__((packed));

// For directory operations
constexpr uint64_t kDotHash = 0;
constexpr uint64_t kDDotHash = kDotHash;
constexpr uint64_t kMaxHash = (~((0x3ULL) << 62));
constexpr uint64_t kHashColBit = ((0x1ULL) << 63);

// One directory entry slot covers 8bytes-long file name
constexpr uint16_t kNameLen = 8;
constexpr uint16_t kNameLenBits = 3;

inline uint16_t GetDentrySlots(uint16_t namelen) {
  return ((namelen + kNameLen - 1) >> kNameLenBits);
}

// the number of dentry in a block
constexpr uint32_t kNrDentryInBlock = 214;

// MAX level for dir lookup
constexpr uint32_t kMaxDirHashDepth = 63;

constexpr size_t kSizeOfDirEntry = 11;  // by byte
constexpr size_t kSizeOfDentryBitmap = (kNrDentryInBlock + kBitsPerByte - 1) >> kShiftForBitSize;
constexpr size_t kSizeOfReserved =
    kBlockSize - ((kSizeOfDirEntry + kNameLen) * kNrDentryInBlock + kSizeOfDentryBitmap);

// One directory entry slot representing kNameLen-sized file name
struct DirEntry {
  uint32_t hash_code = 0;  // hash code of file name
  uint32_t ino = 0;        // inode number
  uint16_t name_len = 0;   // lengh of file name
  uint8_t file_type = 0;   // file type
} __attribute__((packed));

// 4KB-sized directory entry block
struct DentryBlock {
  // validity bitmap for directory entries in each block
  uint8_t dentry_bitmap[kSizeOfDentryBitmap];
  uint8_t reserved[kSizeOfReserved];
  DirEntry dentry[kNrDentryInBlock];
  uint8_t filename[kNrDentryInBlock][kNameLen];
} __attribute__((packed));

// file types used in InodeInfo->flags
enum class FileType {
  kFtUnknown,
  kFtRegFile,
  kFtDir,
  kFtChrdev,
  kFtBlkdev,
  kFtFifo,
  kFtSock,
  kFtSymlink,
  kFtMax,
  kFtOrphan,  // used by fsck
};

constexpr uint32_t kHashBits = 8;

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_LAYOUT_H_
