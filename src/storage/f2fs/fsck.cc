// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/fsck.h"

#include <safemath/checked_math.h>

#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/dir.h"
#include "src/storage/f2fs/f2fs.h"

namespace f2fs {
namespace {
uint32_t MaxInlineData(const Inode &inode) {
  uint16_t extra_isize = 0;
  uint16_t inline_xattr_isize = 0;

  if (inode.i_inline & kExtraAttr) {
    extra_isize = inode.i_extra_isize;
    if (inode.i_inline & kInlineXattr) {
      inline_xattr_isize = inode.i_inline_xattr_size;
    }
  } else if ((inode.i_inline & kInlineXattr) || (inode.i_inline & kInlineDentry)) {
    inline_xattr_isize = kInlineXattrAddrs;
  }

  return sizeof(uint32_t) *
         (kAddrsPerInode - extra_isize / sizeof(uint32_t) - inline_xattr_isize - 1);
}

size_t MaxInlineDentry(const Inode &inode) {
  return GetBitSize(MaxInlineData(inode)) / (GetBitSize(kSizeOfDirEntry + kDentrySlotLen) + 1);
}

const uint8_t *InlineDataPtr(const Inode &inode) {
  uint16_t extra_isize = (inode.i_inline & kExtraAttr) ? inode.i_extra_isize : 0;
  return reinterpret_cast<const uint8_t *>(
      &inode.i_addr[extra_isize / sizeof(uint32_t) + kInlineStartOffset]);
}

const uint8_t *InlineDentryBitmap(const Inode &inode) { return InlineDataPtr(inode); }

const DirEntry *InlineDentryArray(const Inode &inode) {
  size_t reserved =
      MaxInlineData(inode) - MaxInlineDentry(inode) * (kSizeOfDirEntry + kDentrySlotLen);
  return reinterpret_cast<const DirEntry *>(InlineDentryBitmap(inode) + reserved);
}

const uint8_t (*InlineDentryNameArray(const Inode &inode)) [kDentrySlotLen] {
  size_t reserved = MaxInlineData(inode) - MaxInlineDentry(inode) * kDentrySlotLen;
  return reinterpret_cast<const uint8_t (*)[kDentrySlotLen]>(InlineDentryBitmap(inode) + reserved);
}

static bool TestBit(const void *addr, size_t pos, size_t size) {
  if (pos >= size)
    return false;
  const uint8_t *bitmap = static_cast<const uint8_t *>(addr);
  const uint8_t mask = GetMask(uint8_t(1U), pos & kLastNodeMask);
  return (bitmap[pos >> kShiftForBitSize] & mask) != 0;
}

}  // namespace

template <typename T>
static inline void DisplayMember(uint32_t typesize, T value, std::string_view name) {
  if (typesize == sizeof(char)) {
    FX_LOGS(INFO) << "\t" << name << " [" << value << "]";
  } else {
    ZX_ASSERT(sizeof(T) <= typesize);
    FX_LOGS(INFO) << "\t" << name << " [0x" << std::hex << value << " : " << std::dec << value
                  << "]";
  }
}

static int32_t operator-(CursegType &a, CursegType &&b) {
  return (static_cast<int32_t>(a) - static_cast<int32_t>(b));
}

static bool operator<=(int32_t &a, CursegType &&b) { return (a <= static_cast<int32_t>(b)); }

CursegType operator+(CursegType a, uint32_t &&b) {
  return static_cast<CursegType>(static_cast<uint32_t>(a) + b);
}

static inline bool IsSumNodeSeg(SummaryFooter &footer) { return footer.entry_type == kSumTypeNode; }

static inline uint64_t BlkoffFromMain(SegmentManager &manager, uint64_t block_address) {
  ZX_ASSERT(block_address >= manager.GetMainAreaStartBlock());
  return block_address - manager.GetMainAreaStartBlock();
}

static inline uint32_t OffsetInSegment(SuperblockInfo &sbi, SegmentManager &manager,
                                       uint64_t block_address) {
  return static_cast<uint32_t>(BlkoffFromMain(manager, block_address) %
                               (1 << sbi.GetLogBlocksPerSeg()));
}

static inline uint16_t AddrsPerInode(const Inode *i) {
  uint16_t inline_xattr_isize = 0;

  if ((i->i_inline & kExtraAttr) && (i->i_inline & kInlineXattr)) {
    inline_xattr_isize = i->i_inline_xattr_size;
  } else if ((i->i_inline & kInlineXattr) || (i->i_inline & kInlineDentry)) {
    inline_xattr_isize = kInlineXattrAddrs;
  }
  return kAddrsPerInode - inline_xattr_isize;
}

zx_status_t FsckWorker::ReadBlock(void *buffer, block_t bno) { return bc_->Readblk(bno, buffer); }

zx_status_t FsckWorker::WriteBlock(void *buffer, block_t bno) { return bc_->Writeblk(bno, buffer); }

void FsckWorker::AddIntoInodeLinkMap(nid_t nid, uint32_t link_count) {
  auto ret = fsck_.inode_link_map.insert({nid, {.links = link_count, .actual_links = 1}});
  ZX_ASSERT(ret.second);
  if (link_count > 1) {
    FX_LOGS(INFO) << "\tino[0x" << std::hex << nid << "] has hard links [0x" << link_count << "]";
  }
}

zx_status_t FsckWorker::FindAndIncreaseInodeLinkMap(nid_t nid) {
  if (auto ret = fsck_.inode_link_map.find(nid); ret != fsck_.inode_link_map.end()) {
    ++ret->second.actual_links;
    return ZX_OK;
  }
  return ZX_ERR_NOT_FOUND;
}

bool FsckWorker::IsValidSsaNodeBlock(nid_t nid, uint32_t block_address) {
  auto [ret, summary_entry] = GetSummaryEntry(block_address);

  if (ret == SegType::kSegTypeData || ret == SegType::kSegTypeCurData) {
    FX_LOGS(ERROR) << "\tSummary footer is not a node segment summary";
    return false;
  } else if (ret == SegType::kSegTypeNode) {
    if (LeToCpu(summary_entry.nid) != nid) {
      FX_LOGS(ERROR) << "\tnid                       [0x" << std::hex << nid << "]";
      FX_LOGS(ERROR) << "\ttarget block_address           [0x" << std::hex << block_address << "]";
      FX_LOGS(ERROR) << "\tsummary block_address          [0x" << std::hex
                     << segment_manager_->GetSumBlock(
                            segment_manager_->GetSegmentNumber(block_address))
                     << "]";
      FX_LOGS(ERROR) << "\tseg no / offset           [0x" << std::hex
                     << segment_manager_->GetSegmentNumber(block_address) << "/0x" << std::hex
                     << OffsetInSegment(*superblock_info_, *segment_manager_, block_address) << "]";
      FX_LOGS(ERROR) << "\tsummary_entry.nid         [0x" << std::hex << LeToCpu(summary_entry.nid)
                     << "]";
      FX_LOGS(ERROR) << "\t--> node block's nid      [0x" << std::hex << nid << "]";
      FX_LOGS(ERROR) << "\tInvalid node seg summary\n";
      return false;
    }
  } else if (ret == SegType::kSegTypeCurNode) {
    // current node segment has no ssa
  } else {
    FX_LOGS(ERROR) << "\tInvalid return value of 'GetSummaryEntry'";
    return false;
  }
  return true;
}

bool FsckWorker::IsValidSsaDataBlock(uint32_t block_address, uint32_t parent_nid,
                                     uint16_t index_in_node, uint8_t version) {
  auto [ret, summary_entry] = GetSummaryEntry(block_address);
  ZX_ASSERT(ret == SegType::kSegTypeData || ret == SegType::kSegTypeCurData);

  if (LeToCpu(summary_entry.nid) != parent_nid || summary_entry.version != version ||
      LeToCpu(summary_entry.ofs_in_node) != index_in_node) {
    FX_LOGS(ERROR) << "\tsummary_entry.nid         [0x" << std::hex << LeToCpu(summary_entry.nid)
                   << "]";
    FX_LOGS(ERROR) << "\tsummary_entry.version     [0x" << std::hex << summary_entry.version << "]";
    FX_LOGS(ERROR) << "\tsummary_entry.ofs_in_node [0x" << std::hex
                   << LeToCpu(summary_entry.ofs_in_node) << "]";

    FX_LOGS(ERROR) << "\tparent nid                [0x" << std::hex << parent_nid << "]";
    FX_LOGS(ERROR) << "\tversion from nat          [0x" << std::hex << version << "]";
    FX_LOGS(ERROR) << "\tindex in parent node        [0x" << std::hex << index_in_node << "]";

    FX_LOGS(ERROR) << "\tTarget data block address    [0x" << std::hex << block_address << "]";
    FX_LOGS(ERROR) << "\tInvalid data seg summary\n";
    return false;
  }
  return true;
}

bool FsckWorker::IsValidNid(nid_t nid) {
  return nid <= (kNatEntryPerBlock * superblock_info_->GetSuperblock().segment_count_nat
                 << (superblock_info_->GetLogBlocksPerSeg() - 1));
}

bool FsckWorker::IsValidBlockAddress(uint32_t addr) {
  if (addr >= superblock_info_->GetSuperblock().block_count) {
    FX_LOGS(INFO) << "\tblock[0x" << std::hex << addr << "] should be less than [0x"
                  << superblock_info_->GetSuperblock().block_count << "]";
    return false;
  }
  if (addr < segment_manager_->GetMainAreaStartBlock()) {
    FX_LOGS(INFO) << "\tblock[0x" << std::hex << addr << "] should be greater than [0x"
                  << segment_manager_->GetMainAreaStartBlock() << "]";
    return false;
  }
  return true;
}

zx_status_t FsckWorker::ValidateNodeBlock(const Node &node_block, NodeInfo node_info,
                                          FileType ftype, NodeType ntype) {
  if (node_info.nid != LeToCpu(node_block.footer.nid) ||
      node_info.ino != LeToCpu(node_block.footer.ino)) {
    FX_LOGS(ERROR) << "\tino[0x" << std::hex << node_info.ino << "] nid[0x" << node_info.nid
                   << "] blk_addr[0x" << node_info.blk_addr << "] footer.nid[0x"
                   << LeToCpu(node_block.footer.nid) << "] footer.ino[0x"
                   << LeToCpu(node_block.footer.ino) << "]";
    return ZX_ERR_INTERNAL;
  }

  if (ntype == NodeType::kTypeInode) {
    uint32_t i_links = LeToCpu(node_block.i.i_links);

    // Orphan node. i_links should be 0.
    if (ftype == FileType::kFtOrphan) {
      ZX_ASSERT(i_links == 0);
    } else {
      ZX_ASSERT(i_links > 0);
    }
  }

  return ZX_OK;
}

zx::result<bool> FsckWorker::UpdateContext(const Node &node_block, NodeInfo node_info,
                                           FileType ftype, NodeType ntype) {
  nid_t nid = node_info.nid;
  if (ftype != FileType::kFtOrphan || fsck_.nat_area_bitmap.GetOne(ToMsbFirst(nid))) {
    fsck_.nat_area_bitmap.ClearOne(ToMsbFirst(nid));
  } else {
    FX_LOGS(WARNING) << "\tnid duplicated [0x" << std::hex << nid << "]";
  }

  if (!fsck_.main_area_bitmap.GetOne(
          ToMsbFirst(BlkoffFromMain(*segment_manager_, node_info.blk_addr)))) {
    // Unvisited node, mark visited.
    fsck_.main_area_bitmap.SetOne(
        ToMsbFirst(BlkoffFromMain(*segment_manager_, node_info.blk_addr)));
    if (ntype == NodeType::kTypeInode) {
      uint32_t i_links = LeToCpu(node_block.i.i_links);
      if (ftype != FileType::kFtDir && ftype != FileType::kFtOrphan) {
        // First time visiting this inode.
        AddIntoInodeLinkMap(nid, i_links);
        if (i_links > 1) {
          ++fsck_.result.multi_hard_link_files;
        }
      }
      ++fsck_.result.valid_inode_count;
    }

    ++fsck_.result.valid_block_count;
    ++fsck_.result.valid_node_count;
  } else {
    // Once visited here, it should be an Inode.
    if (ntype != NodeType::kTypeInode) {
      FX_LOGS(ERROR) << std::hex << "\tDuplicated node block. nid[0x" << nid << "] blk_addr[0x"
                     << node_info.blk_addr << "]";
      return zx::error(ZX_ERR_INTERNAL);
    }

    uint32_t i_links = LeToCpu(node_block.i.i_links);

    if (ftype == FileType::kFtDir) {
      FX_LOGS(INFO) << "\tDuplicated inode blk. ino[0x" << std::hex << nid << "][0x" << std::hex
                    << node_info.blk_addr;
      return zx::error(ZX_ERR_INTERNAL);
    } else {
      FX_LOGS(INFO) << "\tino[0x" << std::hex << nid << "] has hard links [0x" << std::hex
                    << i_links << "]";
      // We don't go deeper.
      if (auto status = FindAndIncreaseInodeLinkMap(nid); status != ZX_OK) {
        return zx::error(status);
      }
      return zx::ok(false);
    }
  }
  return zx::ok(true);
}

zx::result<NodeInfo> FsckWorker::ReadNodeBlock(nid_t nid, BlockBuffer<Node> &block) {
  if (!IsValidNid(nid)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto result = GetNodeInfo(nid);
  if (result.is_error()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  NodeInfo node_info = *result;

  if (node_info.blk_addr == kNewAddr) {
    FX_LOGS(ERROR) << "\tnid is NEW_ADDR. [0x" << std::hex << nid << "]";
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (!IsValidBlockAddress(node_info.blk_addr) || !IsValidSsaNodeBlock(nid, node_info.blk_addr)) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (!sit_area_bitmap_.GetOne(ToMsbFirst(BlkoffFromMain(*segment_manager_, node_info.blk_addr)))) {
    FX_LOGS(INFO) << "\tSIT bitmap is 0x0. block_address[0x" << std::hex << node_info.blk_addr
                  << "]";
    return zx::error(ZX_ERR_INTERNAL);
  }

  ZX_ASSERT(ReadBlock(&block, node_info.blk_addr) == ZX_OK);

  return zx::ok(node_info);
}

zx::result<TraverseResult> FsckWorker::CheckNodeBlock(const Inode *inode, nid_t nid, FileType ftype,
                                                      NodeType ntype) {
  uint64_t block_count = 0;
  uint32_t link_count = 0;

  auto node_block_ptr = std::make_unique<BlockBuffer<Node>>();
  auto &node_block = *node_block_ptr;
  // Read the node block.
  zx::result node_info = ReadNodeBlock(nid, node_block);
  if (node_info.is_error()) {
    return node_info.take_error();
  }

  // Validate the node block.
  if (auto status = ValidateNodeBlock(*node_block, *node_info, ftype, ntype); status != ZX_OK) {
    return zx::error(status);
  }

  // Update fsck context.
  auto do_traverse = UpdateContext(*node_block, *node_info, ftype, ntype);
  if (do_traverse.is_error()) {
    return do_traverse.take_error();
  }

  if (*do_traverse) {
    // Traverse to underlying structures.
    zx::result<TraverseResult> ret;
    switch (ntype) {
      case NodeType::kTypeInode:
        inode_list_.push_back(std::make_unique<ChildInodeInfo>(nid, ftype));
        ret = zx::ok(TraverseResult{0, 0});
        break;
      case NodeType::kTypeDirectNode:
        ret = TraverseDnodeBlock(inode, *node_block, *node_info, ftype);
        break;
      case NodeType::kTypeIndirectNode:
        ret = TraverseIndirectNodeBlock(inode, *node_block, ftype);
        break;
      case NodeType::kTypeDoubleIndirectNode:
        ret = TraverseDoubleIndirectNodeBlock(inode, *node_block, ftype);
        break;
      default:
        ret = zx::error(ZX_ERR_INTERNAL);
        break;
    }

    if (ret.is_error()) {
      return ret.take_error();
    }
    block_count += ret->block_count;
    link_count += ret->link_count;
  }
  return zx::ok(TraverseResult{block_count, link_count});
}

zx::result<TraverseResult> FsckWorker::TraverseInodeBlock(const Node &node_block,
                                                          NodeInfo node_info, FileType ftype) {
  uint32_t child_count = 0, child_files = 0;
  uint64_t block_count = 1;
  nid_t nid = node_info.nid;
  NodeType ntype;
  uint64_t i_blocks = LeToCpu(node_block.i.i_blocks);

  // ValidateNodeBlock ensures below.
  ZX_ASSERT(node_info.nid == node_info.ino);
  ZX_ASSERT(LeToCpu(node_block.footer.nid) == node_info.nid);
  ZX_ASSERT(LeToCpu(node_block.footer.ino) == node_info.ino);

  zx::result xattr_block = CheckXattrBlock(nid, node_block.i.i_xattr_nid);
  if (xattr_block.is_error()) {
    return xattr_block.take_error();
  }

  if (*xattr_block) {
    ++block_count;
  }

  do {
    if (ftype == FileType::kFtChrdev || ftype == FileType::kFtBlkdev ||
        ftype == FileType::kFtFifo || ftype == FileType::kFtSock) {
      break;
    }

    if (node_block.i.i_inline & kInlineData) {
      if (!(node_block.i.i_inline & kDataExist)) {
        auto zeroes = std::make_unique<char[]>(MaxInlineData(node_block.i));
        if (memcmp(zeroes.get(), InlineDataPtr(node_block.i), MaxInlineData(node_block.i))) {
          FX_LOGS(WARNING) << "\tinode[" << nid << "] has junk inline data";
          fsck_.data_exist_flag_set.insert(nid);
        }
      }
      break;
    }

    if (node_block.i.i_inline & kInlineDentry) {
      if (auto status =
              CheckDentries(child_count, child_files, 1, InlineDentryBitmap(node_block.i),
                            InlineDentryArray(node_block.i), InlineDentryNameArray(node_block.i),
                            MaxInlineDentry(node_block.i));
          status != ZX_OK) {
        return zx::error(status);
      }
    } else {
      uint16_t base =
          (node_block.i.i_inline & kExtraAttr) ? node_block.i.i_extra_isize / sizeof(uint32_t) : 0;

      // check data blocks in inode
      for (uint16_t index = base; index < AddrsPerInode(&node_block.i); ++index) {
        if (LeToCpu(node_block.i.i_addr[index]) != 0) {
          ++block_count;
          if (auto status = CheckDataBlock(LeToCpu(node_block.i.i_addr[index]), child_count,
                                           child_files, (i_blocks == block_count), ftype, nid,
                                           index - base, node_info.version);
              status != ZX_OK) {
            return zx::error(status);
          }
        }
      }
    }

    // check node blocks in inode: direct(2) + indirect(2) + double indirect(1)
    for (int index = 0; index < 5; ++index) {
      if (index == 0 || index == 1) {
        ntype = NodeType::kTypeDirectNode;
      } else if (index == 2 || index == 3) {
        ntype = NodeType::kTypeIndirectNode;
      } else if (index == 4) {
        ntype = NodeType::kTypeDoubleIndirectNode;
      } else {
        ZX_ASSERT(0);
      }

      if (LeToCpu(node_block.i.i_nid[index]) != 0) {
        auto ret = CheckNodeBlock(&node_block.i, LeToCpu(node_block.i.i_nid[index]), ftype, ntype);
        if (ret.is_error()) {
          return ret.take_error();
        }
        block_count += ret->block_count;
        child_count += ret->link_count;
      }
    }
  } while (false);

  uint32_t i_links = LeToCpu(node_block.i.i_links);
  if (i_blocks != block_count) {
    FX_LOGS(WARNING) << "\tfile[0x" << std::hex << nid << "] i_blocks != block_count";
    PrintNodeInfo(node_block);
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (ftype == FileType::kFtDir && i_links != child_count) {
    FX_LOGS(WARNING) << "\tdir[0x" << std::hex << nid << "] i_links " << i_links
                     << " != link_count " << child_count;
    PrintNodeInfo(node_block);
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(TraverseResult{block_count, child_count});
}

zx::result<TraverseResult> FsckWorker::TraverseDnodeBlock(const Inode *inode,
                                                          const Node &node_block,
                                                          NodeInfo node_info, FileType ftype) {
  nid_t nid = node_info.nid;
  uint64_t block_count = 1;
  uint32_t child_count = 0, child_files = 0;
  for (uint16_t index = 0; index < kAddrsPerBlock; ++index) {
    if (LeToCpu(node_block.dn.addr[index]) == 0x0) {
      continue;
    }
    ++block_count;
    if (auto status = CheckDataBlock(LeToCpu(node_block.dn.addr[index]), child_count, child_files,
                                     LeToCpu(inode->i_blocks) == block_count, ftype, nid, index,
                                     node_info.version);
        status != ZX_OK) {
      return zx::error(status);
    }
  }
  return zx::ok(TraverseResult{block_count, child_count});
}

zx::result<TraverseResult> FsckWorker::TraverseIndirectNodeBlock(const Inode *inode,
                                                                 const Node &node_block,
                                                                 FileType ftype) {
  uint64_t block_count = 1;
  uint32_t child_count = 0;
  for (uint32_t child_nid : node_block.in.nid) {
    if (LeToCpu(child_nid) == 0x0) {
      continue;
    }
    auto ret = CheckNodeBlock(inode, LeToCpu(child_nid), ftype, NodeType::kTypeDirectNode);
    if (ret.is_error()) {
      return ret;
    }
    block_count += ret->block_count;
    child_count += ret->link_count;
  }
  return zx::ok(TraverseResult{block_count, child_count});
}

zx::result<TraverseResult> FsckWorker::TraverseDoubleIndirectNodeBlock(const Inode *inode,
                                                                       const Node &node_block,
                                                                       FileType ftype) {
  uint64_t block_count = 1;
  uint32_t child_count = 0;
  for (uint32_t child_nid : node_block.in.nid) {
    if (LeToCpu(child_nid) == 0x0) {
      continue;
    }
    auto ret = CheckNodeBlock(inode, LeToCpu(child_nid), ftype, NodeType::kTypeIndirectNode);
    if (ret.is_error()) {
      return ret;
    }
    block_count += ret->block_count;
    child_count += ret->link_count;
  }
  return zx::ok(TraverseResult{block_count, child_count});
}

zx_status_t FsckWorker::CheckDentries(uint32_t &child_count, uint32_t &child_files,
                                      const int last_block, const uint8_t *dentry_bitmap,
                                      const DirEntry *dentries, const uint8_t (*filename)[kNameLen],
                                      const size_t max_entries) {
  uint32_t hash_code;
  FileType ftype;

  ++fsck_.dentry_depth;

  for (uint32_t i = 0; i < max_entries;) {
    if (!TestBit(dentry_bitmap, i, max_entries)) {
      ++i;
      continue;
    }

    std::string_view name(reinterpret_cast<const char *>(filename[i]),
                          LeToCpu(dentries[i].name_len));
    hash_code = DentryHash(name);

    ftype = static_cast<FileType>(dentries[i].file_type);

    // Becareful. 'dentry.file_type' is not imode
    if (ftype == FileType::kFtDir) {
      ++child_count;
      if (IsDotOrDotDot(name)) {
        ++i;
        continue;
      }
    }

    // Should we check '.' and '..' entries?
    ZX_ASSERT(LeToCpu(dentries[i].hash_code) == hash_code);
    if (fsck_options_.verbose) {
      char str[100];
      std::sprintf(str, "\t[%3u] - no[0x%x] name[%s] len[0x%x] ino[0x%x] type[0x%x]",
                   fsck_.dentry_depth, i, name.data(), LeToCpu(dentries[i].name_len),
                   LeToCpu(dentries[i].ino), dentries[i].file_type);
      FX_LOGS(INFO) << str;
    }

    auto ret = CheckNodeBlock(nullptr, LeToCpu(dentries[i].ino), ftype, NodeType::kTypeInode);
    if (ret.is_error()) {
      return ret.error_value();
    }

    i += CheckedDivRoundUp<uint32_t>(LeToCpu(dentries[i].name_len), kDentrySlotLen);
    ++child_files;
  }

  --fsck_.dentry_depth;
  return ZX_OK;
}

zx_status_t FsckWorker::CheckDentryBlock(uint32_t block_address, uint32_t &child_count,
                                         uint32_t &child_files, int last_block) {
  BlockBuffer<DentryBlock> de_blk;
  ZX_ASSERT(ReadBlock(&de_blk, block_address) == ZX_OK);
  return CheckDentries(child_count, child_files, last_block, de_blk->dentry_bitmap, de_blk->dentry,
                       de_blk->filename, kNrDentryInBlock);
}

zx_status_t FsckWorker::CheckDataBlock(uint32_t block_address, uint32_t &child_count,
                                       uint32_t &child_files, int last_block, FileType ftype,
                                       uint32_t parent_nid, uint16_t index_in_node, uint8_t ver) {
  // Is it reserved block?
  if (block_address == kNewAddr) {
    ++fsck_.result.valid_block_count;
    return ZX_OK;
  }

  if (!IsValidBlockAddress(block_address)) {
    return ZX_ERR_INTERNAL;
  }

  if (!IsValidSsaDataBlock(block_address, parent_nid, index_in_node, ver)) {
    return ZX_ERR_INTERNAL;
  }

  if (!sit_area_bitmap_.GetOne(ToMsbFirst(BlkoffFromMain(*segment_manager_, block_address)))) {
    ZX_ASSERT_MSG(false, "SIT bitmap is 0x0. block_address[0x%x]\n", block_address);
  }

  if (fsck_.main_area_bitmap.GetOne(ToMsbFirst(BlkoffFromMain(*segment_manager_, block_address)))) {
    ZX_ASSERT_MSG(false, "Duplicated data block. pnid[0x%x] index[0x%x] block_address[0x%x]\n",
                  parent_nid, index_in_node, block_address);
  }
  fsck_.main_area_bitmap.SetOne(ToMsbFirst(BlkoffFromMain(*segment_manager_, block_address)));

  ++fsck_.result.valid_block_count;

  if (ftype == FileType::kFtDir) {
    return CheckDentryBlock(block_address, child_count, child_files, last_block);
  }

  return ZX_OK;
}

zx_status_t FsckWorker::TraverseInode(nid_t ino, FileType ftype) {
  auto status = CheckNodeBlock(nullptr, ino, ftype, NodeType::kTypeInode);
  if (status.is_error()) {
    return status.error_value();
  }
  ZX_DEBUG_ASSERT(!inode_list_.is_empty());
  do {
    auto inode_info = inode_list_.pop_front();
    auto node_block = std::make_unique<BlockBuffer<Node>>();
    auto node_info = ReadNodeBlock(inode_info->Id(), *node_block);
    if (node_info.is_error()) {
      return node_info.error_value();
    }
    if (auto ret = TraverseInodeBlock(**node_block, *node_info, inode_info->Ftype());
        ret.is_error()) {
      return ret.error_value();
    }
  } while (!inode_list_.is_empty());
  return ZX_OK;
}

zx_status_t FsckWorker::CheckOrphanNodes() {
  block_t start_blk, orphan_blkaddr;

  if (!superblock_info_->TestCpFlags(CpFlag::kCpOrphanPresentFlag)) {
    return ZX_OK;
  }

  start_blk =
      superblock_info_->StartCpAddr() + 1 + LeToCpu(superblock_info_->GetSuperblock().cp_payload);
  orphan_blkaddr = superblock_info_->StartSumAddr() - 1;

  for (block_t i = 0; i < orphan_blkaddr; ++i) {
    BlockBuffer<OrphanBlock> orphan_block;
    ZX_ASSERT(ReadBlock(&orphan_block, start_blk + i) == ZX_OK);

    for (block_t j = 0; j < LeToCpu(orphan_block->entry_count); ++j) {
      nid_t ino = LeToCpu(orphan_block->ino[j]);
      if (fsck_options_.verbose) {
        FX_LOGS(INFO) << "\t[" << std::hex << i << "] ino [0x" << ino << "]";
      }
      if (zx_status_t ret = TraverseInode(ino, FileType::kFtOrphan); ret != ZX_OK) {
        return ret;
      }
    }
  }
  return ZX_OK;
}

zx::result<bool> FsckWorker::CheckXattrBlock(uint32_t ino, uint32_t x_nid) {
  if (x_nid == 0x0) {
    return zx::ok(false);
  }

  if (fsck_.nat_area_bitmap.GetOne(ToMsbFirst(x_nid)) != 0x0) {
    fsck_.nat_area_bitmap.ClearOne(ToMsbFirst(x_nid));
  } else {
    ZX_ASSERT_MSG(false, "xattr_nid duplicated [0x%x]\n", x_nid);
  }

  ++fsck_.result.valid_block_count;
  ++fsck_.result.valid_node_count;

  zx::result<NodeInfo> result = GetNodeInfo(x_nid);
  if (result.is_error()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  NodeInfo node_info = *result;

  if (fsck_.main_area_bitmap.GetOne(
          ToMsbFirst(BlkoffFromMain(*segment_manager_, node_info.blk_addr))) != 0) {
    ZX_ASSERT_MSG(false,
                  "Duplicated node block for x_attr. "
                  "x_nid[0x%x] block addr[0x%x]\n",
                  x_nid, node_info.blk_addr);
  }
  fsck_.main_area_bitmap.SetOne(ToMsbFirst(BlkoffFromMain(*segment_manager_, node_info.blk_addr)));

  FX_LOGS(INFO) << "ino[0x" << std::hex << ino << "] x_nid[0x" << x_nid << "]";
  return zx::ok(true);
}

zx_status_t FsckWorker::Init() {
  fsck_ = FsckInfo{};
  fsck_.nr_main_blocks = segment_manager_->GetMainSegmentsCount()
                         << superblock_info_->GetLogBlocksPerSeg();
  fsck_.main_area_bitmap_size = CheckedDivRoundUp<uint64_t>(fsck_.nr_main_blocks, kBitsPerByte);
  ZX_ASSERT(fsck_.main_area_bitmap_size == sit_area_bitmap_size_);
  fsck_.main_area_bitmap.Reset(GetBitSize(fsck_.main_area_bitmap_size));

  BuildNatAreaBitmap();

  return ZX_OK;
}

zx_status_t FsckWorker::VerifyCursegOffset(CursegType segtype) {
  CursegInfo *curseg = segment_manager_->CURSEG_I(segtype);
  if (curseg->next_blkoff >= GetBitSize(kSitVBlockMapSize)) {
    return ZX_ERR_INTERNAL;
  }

  block_t logical_curseg_offset = segment_manager_->GetMainAreaStartBlock() +
                                  curseg->segno * superblock_info_->GetBlocksPerSeg() +
                                  curseg->next_blkoff;

  if (!IsValidBlockAddress(logical_curseg_offset)) {
    return ZX_ERR_INTERNAL;
  }

  if (sit_area_bitmap_.GetOne(
          ToMsbFirst(BlkoffFromMain(*segment_manager_, logical_curseg_offset)))) {
    return ZX_ERR_INTERNAL;
  }

  if (curseg->alloc_type == AllocType::kLFS) {
    for (block_t offset = curseg->next_blkoff + 1; offset < kSitVBlockMapSize; ++offset) {
      block_t logical_offset = segment_manager_->GetMainAreaStartBlock() +
                               curseg->segno * superblock_info_->GetBlocksPerSeg() + offset;

      if (sit_area_bitmap_.GetOne(ToMsbFirst(BlkoffFromMain(*segment_manager_, logical_offset)))) {
        return ZX_ERR_INTERNAL;
      }
    }
  }

  return ZX_OK;
}

zx_status_t FsckWorker::Verify() {
  zx_status_t status = ZX_OK;
  uint32_t nr_unref_nid = 0;

  for (uint32_t i = 0; i < fsck_.nr_nat_entries; ++i) {
    if (fsck_.nat_area_bitmap.GetOne(ToMsbFirst(i))) {
      ++nr_unref_nid;
    }
  }

  auto iter = fsck_.inode_link_map.begin();
  while (iter != fsck_.inode_link_map.end()) {
    if (iter->second.links == iter->second.actual_links) {
      iter = fsck_.inode_link_map.erase(iter);
    } else {
      ++iter;
    }
  }

  for (auto const [nid, links] : fsck_.inode_link_map) {
    FX_LOGS(INFO) << "\tnode[0x" << std::hex << nid << "] has inconsistent links [0x" << links.links
                  << "] (actual: 0x" << links.actual_links << ")";
  }

  FX_LOGS(INFO) << "start checking f2fs";
  std::string str = "\tunreachable nat entries\t";
  if (nr_unref_nid == 0x0) {
    FX_LOGS(INFO) << str << "[OK]";
  } else {
    FX_LOGS(INFO) << str << "[FAILED] : " << std::to_string(nr_unref_nid) << " nodes";
    status = ZX_ERR_INTERNAL;
  }

  str = "\tSIT valid block bitmap ";
  if (memcmp(sit_area_bitmap_.StorageUnsafe()->GetData(),
             fsck_.main_area_bitmap.StorageUnsafe()->GetData(), sit_area_bitmap_size_) == 0x0) {
    FX_LOGS(INFO) << str << "[OK]";
  } else {
    FX_LOGS(INFO) << str << "[FAILED]";
    status = ZX_ERR_INTERNAL;
  }

  str = "\thard link count ";
  if (fsck_.inode_link_map.empty()) {
    FX_LOGS(INFO) << str << "[OK] : " << fsck_.result.multi_hard_link_files;
  } else {
    FX_LOGS(INFO) << str << "[FAILED] : " << fsck_.result.multi_hard_link_files;
    status = ZX_ERR_INTERNAL;
  }

  str = "\tvalid_block_count ";
  if (superblock_info_->GetValidBlockCount() == fsck_.result.valid_block_count) {
    FX_LOGS(INFO) << str << "[OK] : " << fsck_.result.valid_block_count;
  } else {
    FX_LOGS(INFO) << str << "[FAILED] : " << superblock_info_->GetValidBlockCount()
                  << "(ckpt) != " << fsck_.result.valid_block_count;
    status = ZX_ERR_INTERNAL;
  }

  str = "\tvalid_node_count[de] ";
  if (superblock_info_->GetValidNodeCount() == fsck_.result.valid_node_count) {
    FX_LOGS(INFO) << str << "[OK] : " << fsck_.result.valid_node_count;
  } else {
    FX_LOGS(INFO) << str << "[FAILED] : " << superblock_info_->GetValidNodeCount()
                  << "(expected) != " << fsck_.result.valid_node_count;
    status = ZX_ERR_INTERNAL;
  }

  str = "\tvalid_node_count[nat] ";
  if (superblock_info_->GetValidNodeCount() == fsck_.result.valid_nat_entry_count) {
    FX_LOGS(INFO) << str << "[OK] : " << fsck_.result.valid_nat_entry_count;
  } else {
    FX_LOGS(INFO) << str << "[FAILED] : " << superblock_info_->GetValidNodeCount()
                  << "(expected) != " << fsck_.result.valid_nat_entry_count;
    status = ZX_ERR_INTERNAL;
  }

  str = "\tvalid_inode_count ";
  if (superblock_info_->GetValidInodeCount() == fsck_.result.valid_inode_count) {
    FX_LOGS(INFO) << str << "[OK] : " << fsck_.result.valid_inode_count;
  } else {
    FX_LOGS(INFO) << str << "[FAILED] : " << superblock_info_->GetValidInodeCount()
                  << "(ckpt) != " << fsck_.result.valid_inode_count;
    status = ZX_ERR_INTERNAL;
  }

  str = "\tnext_blkoff in curseg ";
  std::string segnums;
  bool is_free = true;
  for (uint32_t segtype = 0; segtype < kNrCursegType; ++segtype) {
    if (VerifyCursegOffset(static_cast<CursegType>(segtype)) != ZX_OK) {
      segnums += std::to_string(segtype);
      segnums += " ";
      is_free = false;
    }
  }
  if (is_free) {
    FX_LOGS(INFO) << str << "[OK]";
  } else {
    FX_LOGS(INFO) << str << "[FAILED] : "
                  << "corrupted cursegs(" << segnums << ")";
    status = ZX_ERR_INTERNAL;
  }

  str = "\tinline data in inodes";
  if (fsck_.data_exist_flag_set.empty()) {
    FX_LOGS(INFO) << str << "[OK]";
  } else {
    FX_LOGS(INFO) << str << "[FAILED]";
    status = ZX_ERR_INTERNAL;
  }

  return status;
}

zx_status_t FsckWorker::RepairNat() {
  CursegInfo *curseg = segment_manager_->CURSEG_I(CursegType::kCursegHotData);
  BlockBuffer<SummaryBlock> &summary_block = curseg->sum_blk;

  bool need_journal_update = false;

  for (nid_t nid = 0; nid < fsck_.nr_nat_entries; ++nid) {
    if (fsck_.nat_area_bitmap.GetOne(ToMsbFirst(nid))) {
      FX_LOGS(INFO) << std::hex << "\tRemoving unreachable node [0x" << nid << "]";

      // Lookup the journal first.
      bool found = false;
      for (int i = 0; i < NatsInCursum(*summary_block); ++i) {
        if (LeToCpu(NidInJournal(*summary_block, i)) == nid) {
          // If found, bring in the last entry.
          summary_block->nat_j.entries[i].nid =
              summary_block->nat_j.entries[LeToCpu(summary_block->n_nats) - 1].nid;
          summary_block->n_nats =
              CpuToLe(static_cast<uint16_t>(LeToCpu(summary_block->n_nats) - 1));

          need_journal_update = true;
          found = true;
          break;
        }
      }
      if (found) {
        continue;
      }

      // If not found, go for the NAT.
      block_t block_off = nid / kNatEntryPerBlock;
      uint32_t entry_off = nid % kNatEntryPerBlock;
      block_t seg_off = block_off >> superblock_info_->GetLogBlocksPerSeg();
      block_t block_addr = (node_manager_->GetNatAddress() +
                            (seg_off << superblock_info_->GetLogBlocksPerSeg() << 1) +
                            (block_off & ((1 << superblock_info_->GetLogBlocksPerSeg()) - 1)));

      if (node_manager_->GetNatBitmap().GetOne(ToMsbFirst(block_off))) {
        block_addr += superblock_info_->GetBlocksPerSeg();
      }

      BlockBuffer<NatBlock> nat_block;
      ZX_ASSERT(ReadBlock(&nat_block, block_addr) == ZX_OK);
      nat_block->entries[entry_off] = RawNatEntry{};
      if (auto status = WriteBlock(&nat_block, block_addr); status != ZX_OK) {
        return status;
      }
    }
  }

  if (need_journal_update) {
    if (superblock_info_->TestCpFlags(CpFlag::kCpCompactSumFlag)) {
      block_t summary_addr = StartSummaryBlock();
      BlockBuffer fs_block;
      ReadBlock(&fs_block, summary_addr);
      memcpy(&fs_block, &summary_block->n_nats, kSumJournalSize);
      return WriteBlock(&fs_block, summary_addr);
    } else {
      if (superblock_info_->TestCpFlags(CpFlag::kCpUmountFlag)) {
        return WriteBlock(
            &summary_block,
            SummaryBlockAddress(kNrCursegType, static_cast<int>(CursegType::kCursegHotData)));
      } else {
        return WriteBlock(
            &summary_block,
            SummaryBlockAddress(kNrCursegDataType, static_cast<int>(CursegType::kCursegHotData)));
      }
    }
  }
  return ZX_OK;
}

zx_status_t FsckWorker::RepairSit() {
  SitInfo &sit_i = segment_manager_->GetSitInfo();
  CursegInfo *curseg = segment_manager_->CURSEG_I(CursegType::kCursegColdData);
  BlockBuffer<SummaryBlock> &summary_block = curseg->sum_blk;

  bool need_journal_update = false;
  uint8_t *raw_sit_bitmap = static_cast<uint8_t *>(sit_area_bitmap_.StorageUnsafe()->GetData());
  uint8_t *raw_main_bitmap =
      static_cast<uint8_t *>(fsck_.main_area_bitmap.StorageUnsafe()->GetData());
  for (uint32_t segno = 0; segno < sit_area_bitmap_size_ / kSitVBlockMapSize; ++segno) {
    uint32_t sit_byte_offset = segno * kSitVBlockMapSize;
    if (memcmp(raw_sit_bitmap + sit_byte_offset, raw_main_bitmap + sit_byte_offset,
               std::min(kSitVBlockMapSize, sit_area_bitmap_size_ - sit_byte_offset)) == 0x0) {
      continue;
    }

    // Lookup the journal first.
    bool found = false;
    for (int i = 0; i < SitsInCursum(*summary_block); ++i) {
      if (LeToCpu(SegnoInJournal(*summary_block, i)) == segno) {
        SitEntry &sit = summary_block->sit_j.entries[i].se;
        memcpy(sit.valid_map, raw_main_bitmap + sit_byte_offset, kSitVBlockMapSize);
        sit.vblocks = 0;
        for (uint8_t valid_bits : sit.valid_map) {
          sit.vblocks += std::bitset<8>(valid_bits).count();
        }

        need_journal_update = true;
        found = true;
        break;
      }
    }
    if (found) {
      continue;
    }

    // If not found in journal, go for the Sit.
    std::unique_ptr<BlockBuffer<SitBlock>> sit_block = GetCurrentSitPage(segno);
    uint32_t offset = SitBlockOffset(segno);
    block_t sit_block_addr = sit_i.sit_base_addr + offset;

    if (sit_i.sit_bitmap.GetOne(ToMsbFirst(offset))) {
      sit_block_addr += sit_i.sit_blocks;
    }

    SitEntry &sit_entry = (*sit_block)->entries[segment_manager_->SitEntryOffset(segno)];
    memcpy(sit_entry.valid_map, raw_main_bitmap + sit_byte_offset, kSitVBlockMapSize);
    sit_entry.vblocks = 0;
    for (uint8_t valid_bits : sit_entry.valid_map) {
      sit_entry.vblocks += std::bitset<8>(valid_bits).count();
    }

    if (auto status = WriteBlock(sit_block->get(), sit_block_addr); status != ZX_OK) {
      return status;
    }
  }

  if (need_journal_update) {
    // Write the summary.
    if (superblock_info_->TestCpFlags(CpFlag::kCpCompactSumFlag)) {
      block_t summary_addr = StartSummaryBlock();
      BlockBuffer<uint8_t> fs_block;
      ReadBlock(&fs_block, summary_addr);
      memcpy(fs_block.get<uint8_t>() + kSumJournalSize, &summary_block->n_sits, kSumJournalSize);
      return WriteBlock(&fs_block, summary_addr);
    } else {
      if (superblock_info_->TestCpFlags(CpFlag::kCpUmountFlag)) {
        return WriteBlock(
            &summary_block,
            SummaryBlockAddress(kNrCursegType, static_cast<int>(CursegType::kCursegColdData)));
      } else {
        return WriteBlock(
            &summary_block,
            SummaryBlockAddress(kNrCursegDataType, static_cast<int>(CursegType::kCursegColdData)));
      }
    }
  }
  return ZX_OK;
}

zx_status_t FsckWorker::RepairCheckpoint() {
  bool need_update_checkpoint = false;
  auto &ckpt_block = superblock_info_->GetCheckpointBlock();
  if (superblock_info_->GetValidBlockCount() != fsck_.result.valid_block_count) {
    ckpt_block->valid_block_count = fsck_.result.valid_block_count;
    need_update_checkpoint = true;
  }

  if (superblock_info_->GetValidNodeCount() != fsck_.result.valid_node_count) {
    ckpt_block->valid_node_count = fsck_.result.valid_node_count;
    need_update_checkpoint = true;
  }

  if (superblock_info_->GetValidInodeCount() != fsck_.result.valid_inode_count) {
    ckpt_block->valid_inode_count = fsck_.result.valid_inode_count;
    need_update_checkpoint = true;
  }

  for (uint32_t segtype = 0; segtype < kNrCursegType; ++segtype) {
    CursegInfo *curseg = segment_manager_->CURSEG_I(static_cast<CursegType>(segtype));
    if (VerifyCursegOffset(static_cast<CursegType>(segtype)) != ZX_OK) {
      uint16_t offset;
      for (offset = 0; offset < GetBitSize(kSitVBlockMapSize); ++offset) {
        block_t logical_offset = segment_manager_->GetMainAreaStartBlock() +
                                 curseg->segno * superblock_info_->GetBlocksPerSeg() + offset;
        if (!sit_area_bitmap_.GetOne(
                ToMsbFirst(BlkoffFromMain(*segment_manager_, logical_offset)))) {
          break;
        }
      }

      if (segtype < static_cast<uint32_t>(CursegType::kCursegHotNode)) {
        ckpt_block->cur_data_blkoff[segtype] = offset;
      } else {
        ckpt_block->cur_node_blkoff[segtype - static_cast<uint32_t>(CursegType::kCursegHotNode)] =
            offset;
      }
      ckpt_block->alloc_type[segtype] = static_cast<uint8_t>(AllocType::kSSR);
      need_update_checkpoint = true;
    }
  }

  if (need_update_checkpoint) {
    BlockBuffer<Checkpoint> checkpoint(ckpt_block);
    uint32_t crc =
        F2fsCalCrc32(kF2fsSuperMagic, checkpoint.get(), LeToCpu(checkpoint->checksum_offset));
    *(reinterpret_cast<uint32_t *>(checkpoint.get<uint8_t>() +
                                   LeToCpu(checkpoint->checksum_offset))) = crc;

    if (auto status = WriteBlock(&checkpoint, superblock_info_->StartCpAddr()); status != ZX_OK) {
      return status;
    }
    if (auto status = WriteBlock(&checkpoint, superblock_info_->StartCpAddr() +
                                                  checkpoint->cp_pack_total_block_count - 1);
        status != ZX_OK) {
      return status;
    }
  }
  return ZX_OK;
}

zx_status_t FsckWorker::RepairInodeLinks() {
  for (auto const [nid, links] : fsck_.inode_link_map) {
    BlockBuffer<Node> node_block;
    zx::result node_info = ReadNodeBlock(nid, node_block);
    if (node_info.is_error()) {
      return ZX_ERR_INTERNAL;
    }

    node_block->i.i_links = CpuToLe(links.actual_links);
    if (WriteBlock(&node_block, node_info->blk_addr) != ZX_OK) {
      return ZX_ERR_INTERNAL;
    }
  }
  return ZX_OK;
}

zx_status_t FsckWorker::RepairDataExistFlag() {
  for (auto const nid : fsck_.data_exist_flag_set) {
    BlockBuffer<Node> node_block;
    zx::result node_info = ReadNodeBlock(nid, node_block);
    if (node_info.is_error()) {
      return ZX_ERR_INTERNAL;
    }

    node_block->i.i_inline |= kDataExist;
    if (WriteBlock(&node_block, node_info->blk_addr) != ZX_OK) {
      return ZX_ERR_INTERNAL;
    }
  }
  return ZX_OK;
}

zx_status_t FsckWorker::Repair() {
  if (auto ret = RepairNat(); ret != ZX_OK) {
    return ret;
  }
  if (auto ret = RepairSit(); ret != ZX_OK) {
    return ret;
  }
  if (auto ret = RepairCheckpoint(); ret != ZX_OK) {
    return ret;
  }
  if (auto ret = RepairInodeLinks(); ret != ZX_OK) {
    return ret;
  }
  if (auto ret = RepairDataExistFlag(); ret != ZX_OK) {
    return ret;
  }
  return ZX_OK;
}

void FsckWorker::PrintInodeInfo(const Inode &inode) {
  uint32_t namelen = LeToCpu(inode.i_namelen);

  DisplayMember(sizeof(uint32_t), inode.i_mode, "i_mode");
  DisplayMember(sizeof(uint32_t), inode.i_uid, "i_uid");
  DisplayMember(sizeof(uint32_t), inode.i_gid, "i_gid");
  DisplayMember(sizeof(uint32_t), inode.i_links, "i_links");
  DisplayMember(sizeof(uint64_t), inode.i_size, "i_size");
  DisplayMember(sizeof(uint64_t), inode.i_blocks, "i_blocks");

  DisplayMember(sizeof(uint64_t), inode.i_atime, "i_atime");
  DisplayMember(sizeof(uint32_t), inode.i_atime_nsec, "i_atime_nsec");
  DisplayMember(sizeof(uint64_t), inode.i_ctime, "i_ctime");
  DisplayMember(sizeof(uint32_t), inode.i_ctime_nsec, "i_ctime_nsec");
  DisplayMember(sizeof(uint64_t), inode.i_mtime, "i_mtime");
  DisplayMember(sizeof(uint32_t), inode.i_mtime_nsec, "i_mtime_nsec");

  DisplayMember(sizeof(uint32_t), inode.i_generation, "i_generation");
  DisplayMember(sizeof(uint32_t), inode.i_current_depth, "i_current_depth");
  DisplayMember(sizeof(uint32_t), inode.i_xattr_nid, "i_xattr_nid");
  DisplayMember(sizeof(uint32_t), inode.i_flags, "i_flags");
  DisplayMember(sizeof(uint32_t), inode.i_pino, "i_pino");

  if (namelen) {
    char name[kMaxNameLen];
    DisplayMember(sizeof(uint32_t), inode.i_namelen, "i_namelen");
    std::memcpy(name, inode.i_name, kMaxNameLen);
    name[std::min(namelen, kMaxNameLen)] = '\0';
    DisplayMember(sizeof(char), name, "i_name");
  }

  FX_LOGS(INFO) << "\ti_ext: fofs: " << std::hex << inode.i_ext.fofs
                << ", blkaddr: " << inode.i_ext.blk_addr << ", len: " << inode.i_ext.len;

  DisplayMember(sizeof(uint32_t), inode.i_addr[0], "i_addr[0]");  // Pointers to data blocks
  DisplayMember(sizeof(uint32_t), inode.i_addr[1], "i_addr[1]");  // Pointers to data blocks
  DisplayMember(sizeof(uint32_t), inode.i_addr[2], "i_addr[2]");  // Pointers to data blocks
  DisplayMember(sizeof(uint32_t), inode.i_addr[3], "i_addr[3]");  // Pointers to data blocks

  for (uint32_t i = 4; i < AddrsPerInode(&inode); ++i) {
    if (inode.i_addr[i] != 0x0) {
      FX_LOGS(INFO) << "\ti_addr[0x" << std::hex << i << "] points data block [0x"
                    << inode.i_addr[i] << "]";
      break;
    }
  }

  DisplayMember(sizeof(uint32_t), inode.i_nid[0], "i_nid[0]");  // direct
  DisplayMember(sizeof(uint32_t), inode.i_nid[1], "i_nid[1]");  // direct
  DisplayMember(sizeof(uint32_t), inode.i_nid[2], "i_nid[2]");  // indirect
  DisplayMember(sizeof(uint32_t), inode.i_nid[3], "i_nid[3]");  // indirect
  DisplayMember(sizeof(uint32_t), inode.i_nid[4], "i_nid[4]");  // double indirect
}

void FsckWorker::PrintNodeInfo(const Node &node_block) {
  nid_t ino = LeToCpu(node_block.footer.ino);
  nid_t nid = LeToCpu(node_block.footer.nid);
  if (ino == nid) {
    FX_LOGS(INFO) << "\t== node [0x" << std::hex << nid << ":" << nid << "] is inode ==";
    PrintInodeInfo(node_block.i);
  } else {
    uint32_t *dump_blk = (uint32_t *)&node_block;
    FX_LOGS(INFO) << "\t== node [0x" << std::hex << nid << ":" << nid
                  << "] is direct or indirect ==";
    for (int i = 0; i <= 10; ++i) {  // MSG (0)
      FX_LOGS(INFO) << "\t[" << i << "]\t\t\t[0x" << dump_blk[i] << " : " << dump_blk[i] << "]";
    }
  }
}

void FsckWorker::PrintRawSuperblockInfo() {
  const Superblock &sb = superblock_info_->GetSuperblock();

  FX_LOGS(INFO) << "==== f2fs superblock ====";

  DisplayMember(sizeof(uint32_t), sb.magic, "magic");
  DisplayMember(sizeof(uint32_t), sb.major_ver, "major_ver");
  DisplayMember(sizeof(uint32_t), sb.minor_ver, "minor_ver");
  DisplayMember(sizeof(uint32_t), sb.log_sectorsize, "log_sectorsize");
  DisplayMember(sizeof(uint32_t), sb.log_sectors_per_block, "log_sectors_per_block");

  DisplayMember(sizeof(uint32_t), sb.log_blocksize, "log_blocksize");
  DisplayMember(sizeof(uint32_t), sb.log_blocks_per_seg, "log_blocks_per_seg");
  DisplayMember(sizeof(uint32_t), sb.segs_per_sec, "segs_per_sec");
  DisplayMember(sizeof(uint32_t), sb.secs_per_zone, "secs_per_zone");
  DisplayMember(sizeof(uint32_t), sb.checksum_offset, "checksum_offset");
  DisplayMember(sizeof(uint64_t), sb.block_count, "block_count");

  DisplayMember(sizeof(uint32_t), sb.section_count, "section_count");
  DisplayMember(sizeof(uint32_t), sb.segment_count, "segment_count");
  DisplayMember(sizeof(uint32_t), sb.segment_count_ckpt, "segment_count_ckpt");
  DisplayMember(sizeof(uint32_t), sb.segment_count_sit, "segment_count_sit");
  DisplayMember(sizeof(uint32_t), sb.segment_count_nat, "segment_count_nat");

  DisplayMember(sizeof(uint32_t), sb.segment_count_ssa, "segment_count_ssa");
  DisplayMember(sizeof(uint32_t), sb.segment_count_main, "segment_count_main");
  DisplayMember(sizeof(uint32_t), sb.segment0_blkaddr, "segment0_blkaddr");

  DisplayMember(sizeof(uint32_t), sb.cp_blkaddr, "cp_blkaddr");
  DisplayMember(sizeof(uint32_t), sb.sit_blkaddr, "sit_blkaddr");
  DisplayMember(sizeof(uint32_t), sb.nat_blkaddr, "nat_blkaddr");
  DisplayMember(sizeof(uint32_t), sb.ssa_blkaddr, "ssa_blkaddr");
  DisplayMember(sizeof(uint32_t), sb.main_blkaddr, "main_blkaddr");

  DisplayMember(sizeof(uint32_t), sb.root_ino, "root_ino");
  DisplayMember(sizeof(uint32_t), sb.node_ino, "node_ino");
  DisplayMember(sizeof(uint32_t), sb.meta_ino, "meta_ino");
  DisplayMember(sizeof(uint32_t), sb.cp_payload, "cp_payload");
}

void FsckWorker::PrintCheckpointInfo() {
  const Checkpoint &cp = superblock_info_->GetCheckpoint();
  uint32_t alloc_type;

  FX_LOGS(INFO) << "==== f2fs checkpoint pack ====";

  DisplayMember(sizeof(uint64_t), cp.checkpoint_ver, "checkpoint_ver");
  DisplayMember(sizeof(uint64_t), cp.user_block_count, "user_block_count");
  DisplayMember(sizeof(uint64_t), cp.valid_block_count, "valid_block_count");
  DisplayMember(sizeof(uint32_t), cp.rsvd_segment_count, "rsvd_segment_count");
  DisplayMember(sizeof(uint32_t), cp.overprov_segment_count, "overprov_segment_count");
  DisplayMember(sizeof(uint32_t), cp.free_segment_count, "free_segment_count");

  alloc_type = cp.alloc_type[static_cast<int>(CursegType::kCursegHotNode)];
  DisplayMember(sizeof(uint32_t), alloc_type, "alloc_type[CursegType::kCursegHotNode]");
  alloc_type = cp.alloc_type[static_cast<int>(CursegType::kCursegWarmNode)];
  DisplayMember(sizeof(uint32_t), alloc_type, "alloc_type[CursegType::kCursegWarmNode]");
  alloc_type = cp.alloc_type[static_cast<int>(CursegType::kCursegColdNode)];
  DisplayMember(sizeof(uint32_t), alloc_type, "alloc_type[CursegType::kCursegColdNode]");
  DisplayMember(sizeof(uint32_t), cp.cur_node_segno[0], "cur_node_segno[0]");
  DisplayMember(sizeof(uint32_t), cp.cur_node_segno[1], "cur_node_segno[1]");
  DisplayMember(sizeof(uint32_t), cp.cur_node_segno[2], "cur_node_segno[2]");

  DisplayMember(sizeof(uint32_t), cp.cur_node_blkoff[0], "cur_node_blkoff[0]");
  DisplayMember(sizeof(uint32_t), cp.cur_node_blkoff[1], "cur_node_blkoff[1]");
  DisplayMember(sizeof(uint32_t), cp.cur_node_blkoff[2], "cur_node_blkoff[2]");

  alloc_type = cp.alloc_type[static_cast<int>(CursegType::kCursegHotData)];
  DisplayMember(sizeof(uint32_t), alloc_type, "alloc_type[CursegType::kCursegHotData]");
  alloc_type = cp.alloc_type[static_cast<int>(CursegType::kCursegWarmData)];
  DisplayMember(sizeof(uint32_t), alloc_type, "alloc_type[CursegType::kCursegWarmData]");
  alloc_type = cp.alloc_type[static_cast<int>(CursegType::kCursegColdData)];
  DisplayMember(sizeof(uint32_t), alloc_type, "alloc_type[CursegType::kCursegColdData]");
  DisplayMember(sizeof(uint32_t), cp.cur_data_segno[0], "cur_data_segno[0]");
  DisplayMember(sizeof(uint32_t), cp.cur_data_segno[1], "cur_data_segno[1]");
  DisplayMember(sizeof(uint32_t), cp.cur_data_segno[2], "cur_data_segno[2]");

  DisplayMember(sizeof(uint32_t), cp.cur_data_blkoff[0], "cur_data_blkoff[0]");
  DisplayMember(sizeof(uint32_t), cp.cur_data_blkoff[1], "cur_data_blkoff[1]");
  DisplayMember(sizeof(uint32_t), cp.cur_data_blkoff[2], "cur_data_blkoff[2]");

  DisplayMember(sizeof(uint32_t), cp.ckpt_flags, "ckpt_flags");
  DisplayMember(sizeof(uint32_t), cp.cp_pack_total_block_count, "cp_pack_total_block_count");
  DisplayMember(sizeof(uint32_t), cp.cp_pack_start_sum, "cp_pack_start_sum");
  DisplayMember(sizeof(uint32_t), cp.valid_node_count, "valid_node_count");
  DisplayMember(sizeof(uint32_t), cp.valid_inode_count, "valid_inode_count");
  DisplayMember(sizeof(uint32_t), cp.next_free_nid, "next_free_nid");
  DisplayMember(sizeof(uint32_t), cp.sit_ver_bitmap_bytesize, "sit_ver_bitmap_bytesize");
  DisplayMember(sizeof(uint32_t), cp.nat_ver_bitmap_bytesize, "nat_ver_bitmap_bytesize");
  DisplayMember(sizeof(uint32_t), cp.checksum_offset, "checksum_offset");
  DisplayMember(sizeof(uint64_t), cp.elapsed_time, "elapsed_time");
}

zx_status_t FsckWorker::GetValidSuperblock() {
  if (zx::result superblock = LoadSuperblock(*bc_); superblock.is_error()) {
    return ZX_ERR_NOT_FOUND;
  } else {
    MountOptions options;
    options.SetValue(MountOption::kActiveLogs, kNrCursegType);
    superblock_info_ = std::make_unique<SuperblockInfo>(std::move(*superblock));
  }
  return ZX_OK;
}

zx::result<std::pair<std::unique_ptr<BlockBuffer<Checkpoint>>, uint64_t>>
FsckWorker::ValidateCheckpoint(block_t cp_addr) {
  block_t blk_size = superblock_info_->GetBlocksize();
  uint64_t cur_version = 0, pre_version = 0;
  uint32_t crc = 0;
  uint32_t crc_offset;

  auto cp_block_ptr = std::make_unique<BlockBuffer<Checkpoint>>();
  auto &cp_block = *cp_block_ptr;
  // Read the 1st cp block in this CP pack
  if (ReadBlock(&cp_block, cp_addr) != ZX_OK) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  crc_offset = LeToCpu(cp_block->checksum_offset);
  if (crc_offset >= blk_size) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  crc = *reinterpret_cast<uint32_t *>(cp_block.get<uint8_t>() + crc_offset);
  if (!F2fsCrcValid(crc, cp_block.get(), crc_offset)) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  pre_version = LeToCpu(cp_block->checkpoint_ver);

  // Read the 2nd cp block in this CP pack
  cp_addr += LeToCpu(cp_block->cp_pack_total_block_count) - 1;
  if (ReadBlock(&cp_block, cp_addr) != ZX_OK) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  crc_offset = LeToCpu(cp_block->checksum_offset);
  if (crc_offset >= blk_size) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  crc = *reinterpret_cast<uint32_t *>(cp_block.get<uint8_t>() + crc_offset);
  if (!F2fsCrcValid(crc, cp_block.get(), crc_offset)) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  cur_version = LeToCpu(cp_block->checkpoint_ver);

  if (cur_version == pre_version) {
    return zx::ok(std::pair<std::unique_ptr<BlockBuffer<Checkpoint>>, uint64_t>{
        std::move(cp_block_ptr), cur_version});
  }
  return zx::error(ZX_ERR_INTERNAL);
}

zx_status_t FsckWorker::GetValidCheckpoint() {
  const Superblock &raw_sb = superblock_info_->GetSuperblock();
  zx::result<std::pair<std::unique_ptr<BlockBuffer<Checkpoint>>, uint64_t>> current =
      zx::error(ZX_ERR_NOT_FOUND);
  block_t cp_start_blk_no = 0;

  for (auto checkpoint_start :
       {LeToCpu(raw_sb.cp_blkaddr),
        LeToCpu(raw_sb.cp_blkaddr) + (1 << LeToCpu(raw_sb.log_blocks_per_seg))}) {
    zx::result checkpoint = ValidateCheckpoint(checkpoint_start);
    if (checkpoint.is_error()) {
      continue;
    }

    if (current.is_error() || VerAfter(checkpoint->second, current->second)) {
      current = std::move(checkpoint);
      cp_start_blk_no = checkpoint_start;
    }
  }

  if (current.is_error()) {
    return current.error_value();
  }

  if (zx_status_t status = superblock_info_->SetCheckpoint(*current->first.get());
      status != ZX_OK) {
    return status;
  }

  if (raw_sb.cp_payload) {
    superblock_info_->SetExtraSitBitmap(raw_sb.cp_payload * kBlockSize);
    for (uint32_t i = 0; i < raw_sb.cp_payload; ++i) {
      BlockBuffer blk;
      if (zx_status_t status = ReadBlock(&blk, cp_start_blk_no + 1 + i); status != ZX_OK) {
        return status;
      }
      CloneBits(superblock_info_->GetExtraSitBitmap(), &blk, GetBitSize(i * kBlockSize),
                GetBitSize(kBlockSize));
    }
  }
  return ZX_OK;
}

zx_status_t FsckWorker::InitNodeManager() {
  const Superblock &sb_raw = superblock_info_->GetSuperblock();
  uint32_t nat_segs, nat_blocks;

  node_manager_->SetNatAddress(LeToCpu(sb_raw.nat_blkaddr));

  // segment_count_nat includes pair segment so divide to 2.
  nat_segs = LeToCpu(sb_raw.segment_count_nat) >> 1;
  nat_blocks = nat_segs << LeToCpu(sb_raw.log_blocks_per_seg);
  node_manager_->SetMaxNid(kNatEntryPerBlock * nat_blocks);
  node_manager_->SetNextScanNid(LeToCpu(superblock_info_->GetCheckpoint().next_free_nid));
  if (zx_status_t status = node_manager_->AllocNatBitmap(superblock_info_->GetNatBitmapSize());
      status != ZX_OK) {
    return ZX_ERR_NO_MEMORY;
  }

  // copy version bitmap
  node_manager_->SetNatBitmap(superblock_info_->GetNatBitmap());
  return ZX_OK;
}

zx_status_t FsckWorker::BuildNodeManager() {
  if (node_manager_ = std::make_unique<NodeManager>(superblock_info_.get());
      node_manager_ == nullptr) {
    return ZX_ERR_NO_MEMORY;
  }

  if (zx_status_t err = InitNodeManager(); err != ZX_OK) {
    return err;
  }

  return ZX_OK;
}

zx_status_t FsckWorker::BuildSitInfo() {
  const Superblock &raw_sb = superblock_info_->GetSuperblock();
  const Checkpoint &ckpt = superblock_info_->GetCheckpoint();
  std::unique_ptr<SitInfo> sit_i;
  uint32_t sit_segs;
  uint32_t bitmap_size;

  if (sit_i = std::make_unique<SitInfo>(); sit_i == nullptr) {
    return ZX_ERR_NO_MEMORY;
  }

  sit_i->sentries = std::make_unique<SegmentEntry[]>(segment_manager_->TotalSegs());

  for (uint32_t start = 0; start < segment_manager_->TotalSegs(); ++start) {
    sit_i->sentries[start].cur_valid_map.Reset(GetBitSize(kSitVBlockMapSize));
    sit_i->sentries[start].ckpt_valid_map.Reset(GetBitSize(kSitVBlockMapSize));
  }
  sit_segs = LeToCpu(raw_sb.segment_count_sit) >> 1;
  bitmap_size = superblock_info_->GetSitBitmapSize();
  sit_i->sit_bitmap.Reset(GetBitSize(bitmap_size));
  CloneBits(sit_i->sit_bitmap, superblock_info_->GetSitBitmap(), 0, GetBitSize(bitmap_size));

  sit_i->sit_base_addr = LeToCpu(raw_sb.sit_blkaddr);
  sit_i->sit_blocks = sit_segs << superblock_info_->GetLogBlocksPerSeg();
  sit_i->written_valid_blocks = LeToCpu(safemath::checked_cast<uint32_t>(ckpt.valid_block_count));
  sit_i->bitmap_size = bitmap_size;
  sit_i->dirty_sentries = 0;
  sit_i->elapsed_time = LeToCpu(ckpt.elapsed_time);

  segment_manager_->SetSitInfo(std::move(sit_i));
  return ZX_OK;
}

void FsckWorker::ResetCurseg(CursegType type, int modified) {
  CursegInfo *curseg = segment_manager_->CURSEG_I(type);

  curseg->segno = curseg->next_segno;
  curseg->zone = segment_manager_->GetZoneNoFromSegNo(curseg->segno);
  curseg->next_blkoff = 0;
  curseg->next_segno = kNullSegNo;
}

zx_status_t FsckWorker::ReadCompactedSummaries() {
  const Checkpoint &ckpt = superblock_info_->GetCheckpoint();
  block_t start;
  BlockBuffer<> fs_block;
  uint32_t offset;
  CursegInfo *curseg;

  start = StartSummaryBlock();

  ReadBlock(&fs_block, start++);

  curseg = segment_manager_->CURSEG_I(CursegType::kCursegHotData);
  memcpy(&curseg->sum_blk->n_nats, fs_block.get<uint8_t>(), kSumJournalSize);
  if (NatsInCursum(*curseg->sum_blk) > static_cast<int>(kNatJournalEntries)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  curseg = segment_manager_->CURSEG_I(CursegType::kCursegColdData);
  memcpy(&curseg->sum_blk->n_sits, fs_block.get<uint8_t>() + kSumJournalSize, kSumJournalSize);
  if (SitsInCursum(*curseg->sum_blk) > static_cast<int>(kSitJournalEntries)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  offset = 2 * kSumJournalSize;
  for (int32_t i = static_cast<int32_t>(CursegType::kCursegHotData);
       i <= CursegType::kCursegColdData; ++i) {
    unsigned short blk_off;
    uint32_t segno;

    curseg = segment_manager_->CURSEG_I(static_cast<CursegType>(i));
    segno = LeToCpu(ckpt.cur_data_segno[i]);
    blk_off = LeToCpu(ckpt.cur_data_blkoff[i]);
    if (!segment_manager_->IsValidSegmentNumber(segno) ||
        blk_off >= superblock_info_->GetBlocksPerSeg()) {
      return ZX_ERR_INVALID_ARGS;
    }
    curseg->next_segno = segno;
    ResetCurseg(static_cast<CursegType>(i), 0);
    curseg->alloc_type = static_cast<AllocType>(ckpt.alloc_type[i]);
    curseg->next_blkoff = blk_off;

    if (curseg->alloc_type == AllocType::kSSR) {
      blk_off = safemath::checked_cast<unsigned short>(superblock_info_->GetBlocksPerSeg());
    }

    for (uint32_t j = 0; j < blk_off; ++j) {
      memcpy(&curseg->sum_blk->entries[j], &fs_block.get<uint8_t>()[offset], kSummarySize);
      offset += kSummarySize;
      if (offset + kSummarySize <= kBlockSize - kSumFooterSize) {
        continue;
      }
      memset(&fs_block, 0, kBlockSize);
      ReadBlock(&fs_block, start++);
      offset = 0;
    }
  }

  return ZX_OK;
}

zx_status_t FsckWorker::RestoreNodeSummary(uint32_t segno, SummaryBlock &summary_block) {
  BlockBuffer<Node> node_block;
  block_t addr = segment_manager_->StartBlock(segno);
  // scan the node segment
  for (uint32_t i = 0; i < superblock_info_->GetBlocksPerSeg(); ++i, ++addr) {
    if (ReadBlock(&node_block, addr)) {
      break;
    }
    summary_block.entries[i].nid = node_block->footer.nid;
  }
  return ZX_OK;
}

zx_status_t FsckWorker::ReadNormalSummaries(CursegType type) {
  const Checkpoint &ckpt = superblock_info_->GetCheckpoint();
  unsigned short blk_off;
  uint32_t segno = 0;
  block_t block_address = 0;

  if (IsDataSeg(type)) {
    segno = LeToCpu(ckpt.cur_data_segno[static_cast<int>(type)]);
    blk_off = LeToCpu(ckpt.cur_data_blkoff[type - CursegType::kCursegHotData]);
    if (!segment_manager_->IsValidSegmentNumber(segno) ||
        blk_off >= superblock_info_->GetBlocksPerSeg()) {
      return ZX_ERR_INVALID_ARGS;
    }

    if (superblock_info_->TestCpFlags(CpFlag::kCpUmountFlag)) {
      block_address = SummaryBlockAddress(kNrCursegType, static_cast<int>(type));
    } else {
      block_address = SummaryBlockAddress(kNrCursegDataType, static_cast<int>(type));
    }
  } else {
    segno = LeToCpu(ckpt.cur_node_segno[type - CursegType::kCursegHotNode]);
    blk_off = LeToCpu(ckpt.cur_node_blkoff[type - CursegType::kCursegHotNode]);
    if (!segment_manager_->IsValidSegmentNumber(segno) ||
        blk_off >= superblock_info_->GetBlocksPerSeg()) {
      return ZX_ERR_INVALID_ARGS;
    }

    if (superblock_info_->TestCpFlags(CpFlag::kCpUmountFlag)) {
      block_address = SummaryBlockAddress(kNrCursegNodeType, type - CursegType::kCursegHotNode);
    } else {
      block_address = segment_manager_->GetSumBlock(segno);
    }
  }

  BlockBuffer<SummaryBlock> summary_block;
  ReadBlock(&summary_block, block_address);

  if (IsNodeSeg(type)) {
    if (superblock_info_->TestCpFlags(CpFlag::kCpUmountFlag)) {
#if 0  // do not change original value
      Summary *sum_entry = &sum_blk->entries[0];
      for (uint64_t i = 0; i < superblock_info->GetBlocksPerSeg(); ++i, ++sum_entry) {
				sum_entry->version = 0;
				sum_entry->ofs_in_node = 0;
      }
#endif
    } else {
      if (zx_status_t ret = RestoreNodeSummary(segno, *summary_block); ret != ZX_OK) {
        return ret;
      }
    }
  }

  CursegInfo *curseg = segment_manager_->CURSEG_I(type);
  memcpy(curseg->sum_blk.get(), summary_block.get(), kBlockSize);

  if (type == CursegType::kCursegHotData &&
      NatsInCursum(*curseg->sum_blk) > static_cast<int>(kNatJournalEntries)) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  if (type == CursegType::kCursegColdData &&
      SitsInCursum(*curseg->sum_blk) > static_cast<int>(kSitJournalEntries)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  curseg->next_segno = segno;
  ResetCurseg(type, 0);
  curseg->alloc_type = static_cast<AllocType>(ckpt.alloc_type[static_cast<int>(type)]);
  curseg->next_blkoff = blk_off;

  return ZX_OK;
}

zx_status_t FsckWorker::RestoreCursegSummaries() {
  int32_t type = static_cast<int32_t>(CursegType::kCursegHotData);

  if (superblock_info_->TestCpFlags(CpFlag::kCpCompactSumFlag)) {
    if (zx_status_t ret = ReadCompactedSummaries(); ret != ZX_OK) {
      return ret;
    }
    type = static_cast<int32_t>(CursegType::kCursegHotNode);
  }

  for (; type <= CursegType::kCursegColdNode; ++type) {
    if (zx_status_t ret = ReadNormalSummaries(static_cast<CursegType>(type)); ret != ZX_OK) {
      return ret;
    }
  }
  return ZX_OK;
}

zx_status_t FsckWorker::BuildCurseg() {
  for (int i = 0; i < kNrCursegType; ++i) {
    CursegInfo *curseg = segment_manager_->CURSEG_I(static_cast<CursegType>(i));
    memset(curseg->sum_blk.get(), 0, kBlockSize);
    curseg->segno = kNullSegNo;
    curseg->next_blkoff = 0;
  }
  return RestoreCursegSummaries();
}

std::unique_ptr<BlockBuffer<SitBlock>> FsckWorker::GetCurrentSitPage(uint32_t segno) {
  SitInfo &sit_i = segment_manager_->GetSitInfo();
  uint32_t offset = SitBlockOffset(segno);
  block_t block_address = sit_i.sit_base_addr + offset;
  auto sit_block_ptr = std::make_unique<BlockBuffer<SitBlock>>();

  ZX_ASSERT(segment_manager_->IsValidSegmentNumber(segno));

  // calculate sit block address
  if (sit_i.sit_bitmap.GetOne(ToMsbFirst(offset))) {
    block_address += sit_i.sit_blocks;
  }

  ReadBlock(sit_block_ptr->get(), block_address);

  return sit_block_ptr;
}

void FsckWorker::CheckBlockCount(uint32_t segno, const SitEntry &raw_sit) {
  int valid_blocks = 0;

  // check segment usage
  ZX_ASSERT(GetSitVblocks(raw_sit) <= superblock_info_->GetBlocksPerSeg());

  // check segment type
  ZX_ASSERT(SegmentManager::IsValidSegmentType(GetSitType(raw_sit)));

  // check boundary of a given segment number
  ZX_ASSERT(segment_manager_->IsValidSegmentNumber(segno));

  // check bitmap with valid block count
  for (uint64_t i = 0; i < superblock_info_->GetBlocksPerSeg(); ++i) {
    if (TestBit(raw_sit.valid_map, ToMsbFirst(i), kSitVBlockMapSizeInBit)) {
      ++valid_blocks;
    }
  }
  ZX_ASSERT(GetSitVblocks(raw_sit) == valid_blocks);
}

void FsckWorker::SegmentInfoFromRawSit(SegmentEntry &segment_entry, const SitEntry &raw_sit) {
  segment_entry.valid_blocks = GetSitVblocks(raw_sit);
  segment_entry.ckpt_valid_blocks = GetSitVblocks(raw_sit);
  CloneBits<RawBitmapHeap>(segment_entry.cur_valid_map, raw_sit.valid_map, 0,
                           GetBitSize(kSitVBlockMapSize));
  CloneBits<RawBitmapHeap>(segment_entry.ckpt_valid_map, raw_sit.valid_map, 0,
                           GetBitSize(kSitVBlockMapSize));
  segment_entry.type = GetSitType(raw_sit);
  segment_entry.mtime = LeToCpu(raw_sit.mtime);
}

SegmentEntry &FsckWorker::GetSegmentEntry(uint32_t segno) {
  SitInfo &sit_i = segment_manager_->GetSitInfo();
  return sit_i.sentries[segno];
}

SegType FsckWorker::GetSumBlockInfo(uint32_t segno, BlockBuffer<SummaryBlock> &summary_block) {
  const Checkpoint &ckpt = superblock_info_->GetCheckpoint();
  CursegInfo *curseg;
  block_t ssa_blk;

  ssa_blk = segment_manager_->GetSumBlock(segno);
  for (int type = 0; type < kNrCursegNodeType; ++type) {
    if (segno == ckpt.cur_node_segno[type]) {
      curseg = segment_manager_->CURSEG_I(CursegType::kCursegHotNode + type);
      memcpy(summary_block.get(), curseg->sum_blk.get(), kBlockSize);
      return SegType::kSegTypeCurNode;  // current node seg was not stored
    }
  }

  for (int type = 0; type < kNrCursegDataType; ++type) {
    if (segno == ckpt.cur_data_segno[type]) {
      curseg = segment_manager_->CURSEG_I(CursegType::kCursegHotData + type);
      memcpy(summary_block.get(), curseg->sum_blk.get(), kBlockSize);
      ZX_ASSERT(!IsSumNodeSeg(summary_block->footer));
      if (fsck_options_.verbose) {
        FX_LOGS(INFO) << "\tsegno [0x" << std::hex << segno << "] is current data seg[0x" << type
                      << "]";
      }

      return SegType::kSegTypeCurData;  // current data seg was not stored
    }
  }

  ZX_ASSERT(ReadBlock(&summary_block, ssa_blk) == ZX_OK);

  if (IsSumNodeSeg(summary_block->footer)) {
    return SegType::kSegTypeNode;
  }
  return SegType::kSegTypeData;
}

uint32_t FsckWorker::GetSegmentNumber(uint32_t block_address) {
  return static_cast<uint32_t>(BlkoffFromMain(*segment_manager_, block_address) >>
                               superblock_info_->GetLogBlocksPerSeg());
}

std::pair<SegType, Summary> FsckWorker::GetSummaryEntry(uint32_t block_address) {
  uint32_t segno, offset;
  Summary summary_entry;

  segno = GetSegmentNumber(block_address);
  offset = OffsetInSegment(*superblock_info_, *segment_manager_, block_address);

  BlockBuffer<SummaryBlock> summary_block;
  auto type = GetSumBlockInfo(segno, summary_block);
  memcpy(&summary_entry, &summary_block->entries[offset], sizeof(Summary));
  return {type, summary_entry};
}

zx::result<RawNatEntry> FsckWorker::GetNatEntry(nid_t nid) {
  block_t block_off;
  block_t block_addr;
  block_t seg_off;
  block_t entry_off;

  if ((nid / kNatEntryPerBlock) > fsck_.nr_nat_entries) {
    FX_LOGS(WARNING) << "\tnid is over max nid";
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (auto result = LookupNatInJournal(nid); result.is_ok()) {
    return result;
  }

  block_off = nid / kNatEntryPerBlock;
  entry_off = nid % kNatEntryPerBlock;

  seg_off = block_off >> superblock_info_->GetLogBlocksPerSeg();
  block_addr =
      (node_manager_->GetNatAddress() + (seg_off << superblock_info_->GetLogBlocksPerSeg() << 1) +
       (block_off & ((1 << superblock_info_->GetLogBlocksPerSeg()) - 1)));

  if (node_manager_->GetNatBitmap().GetOne(ToMsbFirst(block_off))) {
    block_addr += superblock_info_->GetBlocksPerSeg();
  }

  BlockBuffer<NatBlock> nat_block;
  ZX_ASSERT(ReadBlock(&nat_block, block_addr) == ZX_OK);
  return zx::ok(nat_block->entries[entry_off]);
}

zx::result<NodeInfo> FsckWorker::GetNodeInfo(nid_t nid) {
  NodeInfo node_info;
  auto result = GetNatEntry(nid);
  if (result.is_error()) {
    return result.take_error();
  }
  RawNatEntry raw_nat = *result;

  node_info.nid = nid;
  NodeInfoFromRawNat(node_info, raw_nat);
  return zx::ok(node_info);
}

void FsckWorker::BuildSitEntries() {
  SitInfo &sit_i = segment_manager_->GetSitInfo();
  CursegInfo *curseg = segment_manager_->CURSEG_I(CursegType::kCursegColdData);
  SummaryBlock *sum = &curseg->sum_blk;

  for (uint32_t segno = 0; segno < segment_manager_->TotalSegs(); ++segno) {
    SegmentEntry &segment_entry = sit_i.sentries[segno];
    SitEntry sit;
    bool found = false;

    for (int i = 0; i < SitsInCursum(*sum); ++i) {
      if (LeToCpu(SegnoInJournal(*sum, i)) == segno) {
        sit = sum->sit_j.entries[i].se;
        found = true;
        break;
      }
    }
    if (!found) {
      std::unique_ptr<BlockBuffer<SitBlock>> sit_block = GetCurrentSitPage(segno);
      sit = (*sit_block)->entries[segment_manager_->SitEntryOffset(segno)];
    }
    CheckBlockCount(segno, sit);
    SegmentInfoFromRawSit(segment_entry, sit);
  }
}

zx_status_t FsckWorker::BuildSegmentManager() {
  const Superblock &raw_super = superblock_info_->GetSuperblock();
  const Checkpoint &ckpt = superblock_info_->GetCheckpoint();

  if (segment_manager_ = std::make_unique<SegmentManager>(*superblock_info_);
      segment_manager_ == nullptr) {
    return ZX_ERR_NO_MEMORY;
  }

  // init sm info
  segment_manager_->SetSegment0StartBlock(LeToCpu(raw_super.segment0_blkaddr));
  segment_manager_->SetMainAreaStartBlock(LeToCpu(raw_super.main_blkaddr));
  segment_manager_->SetSegmentsCount(LeToCpu(raw_super.segment_count));
  segment_manager_->SetReservedSegmentsCount(LeToCpu(ckpt.rsvd_segment_count));
  segment_manager_->SetOPSegmentsCount(LeToCpu(ckpt.overprov_segment_count));
  segment_manager_->SetMainSegmentsCount(LeToCpu(raw_super.segment_count_main));
  segment_manager_->SetSSAreaStartBlock(LeToCpu(raw_super.ssa_blkaddr));

  if (auto status = BuildSitInfo(); status != ZX_OK) {
    return status;
  }
  if (auto status = segment_manager_->BuildFreeSegmap(); status != ZX_OK) {
    return status;
  }
  if (auto status = BuildCurseg(); status != ZX_OK) {
    return status;
  }
  BuildSitEntries();
  return ZX_OK;
}

void FsckWorker::BuildSitAreaBitmap() {
  uint32_t vblocks = 0;
  sit_area_bitmap_size_ = segment_manager_->GetMainSegmentsCount() * kSitVBlockMapSize;
  sit_area_bitmap_.Reset(GetBitSize(sit_area_bitmap_size_));
  uint8_t *raw_bitmap = static_cast<uint8_t *>(sit_area_bitmap_.StorageUnsafe()->GetData());
  for (uint32_t segno = 0; segno < segment_manager_->GetMainSegmentsCount(); ++segno) {
    SegmentEntry &segment_entry = GetSegmentEntry(segno);
    CloneBits<RawBitmapHeap>(raw_bitmap, segment_entry.cur_valid_map, 0,
                             GetBitSize(kSitVBlockMapSize));
    vblocks = 0;
    for (uint64_t j = 0; j < kSitVBlockMapSize; ++j) {
      vblocks += std::bitset<8>(*raw_bitmap++).count();
    }
    ZX_ASSERT(vblocks == segment_entry.valid_blocks);

    if (segment_entry.valid_blocks == 0x0) {
      if (superblock_info_->GetCheckpoint().cur_node_segno[0] == segno ||
          superblock_info_->GetCheckpoint().cur_data_segno[0] == segno ||
          superblock_info_->GetCheckpoint().cur_node_segno[1] == segno ||
          superblock_info_->GetCheckpoint().cur_data_segno[1] == segno ||
          superblock_info_->GetCheckpoint().cur_node_segno[2] == segno ||
          superblock_info_->GetCheckpoint().cur_data_segno[2] == segno) {
        continue;
      }
    } else {
      ZX_ASSERT(segment_entry.valid_blocks <= 512);
    }
  }
}

zx::result<RawNatEntry> FsckWorker::LookupNatInJournal(nid_t nid) {
  RawNatEntry raw_nat;
  CursegInfo *curseg = segment_manager_->CURSEG_I(CursegType::kCursegHotData);
  SummaryBlock *sum = &curseg->sum_blk;

  for (int i = 0; i < NatsInCursum(*sum); ++i) {
    if (LeToCpu(NidInJournal(*sum, i)) == nid) {
      RawNatEntry ret = NatInJournal(*sum, i);
      if (fsck_options_.verbose) {
        FX_LOGS(INFO) << "\tnid [0x" << nid << "] in nat cache";
      }
      memcpy(&raw_nat, &ret, sizeof(RawNatEntry));
      return zx::ok(raw_nat);
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

void FsckWorker::BuildNatAreaBitmap() {
  const Superblock &raw_sb = superblock_info_->GetSuperblock();
  nid_t nid, nr_nat_blks;

  block_t block_off;
  block_t block_addr;
  block_t seg_off;

  // Alloc & build nat entry bitmap
  nr_nat_blks = (LeToCpu(raw_sb.segment_count_nat) / 2) << superblock_info_->GetLogBlocksPerSeg();

  fsck_.nr_nat_entries = nr_nat_blks * kNatEntryPerBlock;
  fsck_.nat_area_bitmap_size = CheckedDivRoundUp(fsck_.nr_nat_entries, kBitsPerByte);
  fsck_.nat_area_bitmap.Reset(GetBitSize(fsck_.nat_area_bitmap_size));

  for (block_off = 0; block_off < nr_nat_blks; ++block_off) {
    seg_off = block_off >> superblock_info_->GetLogBlocksPerSeg();
    block_addr = node_manager_->GetNatAddress() +
                 (seg_off << superblock_info_->GetLogBlocksPerSeg() << 1) +
                 (block_off & ((1 << superblock_info_->GetLogBlocksPerSeg()) - 1));

    if (node_manager_->GetNatBitmap().GetOne(ToMsbFirst(block_off))) {
      block_addr += superblock_info_->GetBlocksPerSeg();
    }

    BlockBuffer<NatBlock> nat_block;
    ZX_ASSERT(ReadBlock(&nat_block, block_addr) == ZX_OK);

    nid = block_off * kNatEntryPerBlock;
    for (uint32_t i = 0; i < kNatEntryPerBlock; ++i) {
      NodeInfo node_info;
      node_info.nid = nid + i;

      if ((nid + i) == superblock_info_->GetNodeIno() ||
          (nid + i) == superblock_info_->GetMetaIno()) {
        ZX_ASSERT(nat_block->entries[i].block_addr != 0x0);
        continue;
      }

      if (auto result = LookupNatInJournal(nid + i); result.is_ok()) {
        RawNatEntry raw_nat = *result;
        NodeInfoFromRawNat(node_info, raw_nat);
        if (node_info.blk_addr != kNullAddr) {
          fsck_.nat_area_bitmap.SetOne(ToMsbFirst(nid + i));
          ++fsck_.result.valid_nat_entry_count;
        }
      } else {
        NodeInfoFromRawNat(node_info, nat_block->entries[i]);
        if (node_info.blk_addr != kNullAddr) {
          ZX_ASSERT(nid + i != 0x0);
          if (fsck_options_.verbose) {
            FX_LOGS(INFO) << "\tnid[0x" << std::hex << nid + i << "] in nat entry [0x"
                          << node_info.blk_addr << "] [0x" << node_info.ino << "]";
          }
          fsck_.nat_area_bitmap.SetOne(ToMsbFirst(nid + i));
          ++fsck_.result.valid_nat_entry_count;
        }
      }
    }
  }
  if (fsck_options_.verbose) {
    FX_LOGS(INFO) << "\tvalid nat entries (block_addr != 0x0) [0x" << std::hex
                  << fsck_.result.valid_nat_entry_count << " : "
                  << fsck_.result.valid_nat_entry_count << "]";
  }
}

zx_status_t FsckWorker::DoMount() {
  if (mounted_) {
    DoUmount();
  }

  if (auto status = GetValidSuperblock(); status != ZX_OK) {
    FX_LOGS(ERROR) << "\tCan't find a valid F2FS superblock: " << status;
    return status;
  }

  if (auto status = GetValidCheckpoint(); status != ZX_OK) {
    FX_LOGS(ERROR) << "\tCan't find valid checkpoint: " << status;
    return status;
  }

  if (auto status = BuildSegmentManager(); status != ZX_OK) {
    FX_LOGS(ERROR) << "\tbuild_segment_manager failed: " << status;
    return status;
  }
  if (auto status = BuildNodeManager(); status != ZX_OK) {
    FX_LOGS(ERROR) << "\tbuild_segment_manager failed: " << status;
    return status;
  }

  BuildSitAreaBitmap();

  mounted_ = true;
  return ZX_OK;
}

void FsckWorker::DoUmount() {
  if (!mounted_) {
    return;
  }

  node_manager_.reset();
  segment_manager_.reset();
  mounted_ = false;
}

zx_status_t FsckWorker::DoFsck() {
  if (auto status = Init(); status != ZX_OK) {
    return status;
  }

  if (auto status = CheckOrphanNodes(); status != ZX_OK) {
    return status;
  }

  // Traverse all inodes from root inode
  if (zx_status_t ret = TraverseInode(superblock_info_->GetRootIno(), FileType::kFtDir);
      ret != ZX_OK) {
    return ret;
  }

  if (auto status = Verify(); status != ZX_OK) {
    if (fsck_options_.repair) {
      status = Repair();
      FX_LOGS(INFO) << "repairing f2fs " << (status == ZX_OK ? "[OK]" : "[FAILED]");
    }
    return status;
  }
  return ZX_OK;
}

zx_status_t FsckWorker::Run() {
  zx_status_t status = ZX_OK;
  if (status = DoMount(); status != ZX_OK) {
    return status;
  }

  status = DoFsck();
  if (status != ZX_OK) {
    FX_LOGS(INFO) << "f2fs corruption detected";
    PrintRawSuperblockInfo();
    PrintCheckpointInfo();
  }
  return status;
}

zx_status_t Fsck(std::unique_ptr<BcacheMapper> bc, const FsckOptions &options,
                 std::unique_ptr<BcacheMapper> *out) {
  zx_status_t status;
  FsckWorker fsck(std::move(bc), options);
  status = fsck.Run();
  if (out != nullptr) {
    *out = fsck.Destroy();
  }
  return status;
}

}  // namespace f2fs
