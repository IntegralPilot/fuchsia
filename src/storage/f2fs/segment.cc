// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/segment.h"

#include <safemath/checked_math.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vnode.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {
static CursegType GetSegmentType2(Page &page, PageType p_type) {
  if (p_type == PageType::kData) {
    return CursegType::kCursegHotData;
  } else {
    return CursegType::kCursegHotNode;
  }
}

static CursegType GetSegmentType4(Page &page, PageType p_type) {
  if (p_type == PageType::kData) {
    if (page.GetVnode().IsDir()) {
      return CursegType::kCursegHotData;
    }
    return CursegType::kCursegColdData;
  }

  ZX_ASSERT(p_type == PageType::kNode);
  NodePage *node_page = static_cast<NodePage *>(&page);
  if (node_page->IsDnode() && !node_page->IsColdNode()) {
    return CursegType::kCursegHotNode;
  }
  return CursegType::kCursegColdNode;
}

static CursegType GetSegmentType6(Page &page, PageType p_type) {
  if (p_type == PageType::kData) {
    VnodeF2fs &vnode = page.GetVnode();

    if (vnode.IsDir()) {
      return CursegType::kCursegHotData;
    } else if (page.IsColdData() || vnode.IsColdFile()) {
      return CursegType::kCursegColdData;
    }
    return CursegType::kCursegWarmData;
  }
  ZX_ASSERT(p_type == PageType::kNode);
  NodePage *node_page = static_cast<NodePage *>(&page);
  if (node_page->IsDnode()) {
    return node_page->IsColdNode() ? CursegType::kCursegWarmNode : CursegType::kCursegHotNode;
  }
  return CursegType::kCursegColdNode;
}

CursegType GetSegmentType(Page &page, PageType p_type, size_t num_logs) {
  switch (num_logs) {
    case 2:
      return GetSegmentType2(page, p_type);
    case 4:
      return GetSegmentType4(page, p_type);
    case 6:
      return GetSegmentType6(page, p_type);
    default:
      ZX_ASSERT(0);
  }
}

static void SegInfoFromRawSit(SegmentEntry &segment_entry, SitEntry &raw_sit) {
  segment_entry.valid_blocks = GetSitVblocks(raw_sit);
  segment_entry.ckpt_valid_blocks = GetSitVblocks(raw_sit);
  segment_entry.ckpt_invalid_blocks = kDefaultBlocksPerSegment - segment_entry.ckpt_valid_blocks;
  CloneBits<RawBitmapHeap>(segment_entry.cur_valid_map, raw_sit.valid_map, 0,
                           GetBitSize(kSitVBlockMapSize));
  CloneBits<RawBitmapHeap>(segment_entry.ckpt_valid_map, raw_sit.valid_map, 0,
                           GetBitSize(kSitVBlockMapSize));
  segment_entry.type = GetSitType(raw_sit);
  segment_entry.mtime = LeToCpu(uint64_t{raw_sit.mtime});
}

static void SegInfoToRawSit(SegmentEntry &segment_entry, SitEntry &raw_sit) {
  uint16_t raw_vblocks =
      static_cast<uint16_t>(segment_entry.type << kSitVblocksShift) | segment_entry.valid_blocks;
  raw_sit.vblocks = CpuToLe(raw_vblocks);
  CloneBits<RawBitmapHeap>(raw_sit.valid_map, segment_entry.cur_valid_map, 0,
                           GetBitSize(kSitVBlockMapSize));
  CloneBits<RawBitmapHeap>(segment_entry.ckpt_valid_map, raw_sit.valid_map, 0,
                           GetBitSize(kSitVBlockMapSize));
  segment_entry.ckpt_valid_blocks = segment_entry.valid_blocks;
  segment_entry.ckpt_invalid_blocks = kDefaultBlocksPerSegment - segment_entry.ckpt_valid_blocks;
  raw_sit.mtime = CpuToLe(static_cast<uint64_t>(segment_entry.mtime));
}

int UpdateNatsInCursum(SummaryBlock &raw_summary, int i) {
  int n_nats = NatsInCursum(raw_summary);
  raw_summary.n_nats = CpuToLe(safemath::checked_cast<uint16_t>(n_nats + i));
  return n_nats;
}

static int UpdateSitsInCursum(SummaryBlock &raw_summary, int i) {
  int n_sits = SitsInCursum(raw_summary);
  raw_summary.n_sits = CpuToLe(safemath::checked_cast<uint16_t>(n_sits + i));
  return n_sits;
}

int LookupJournalInCursum(SummaryBlock &sum, JournalType type, uint32_t val, int alloc) {
  if (type == JournalType::kNatJournal) {
    for (int i = 0; i < NatsInCursum(sum); ++i) {
      if (LeToCpu(NidInJournal(sum, i)) == val)
        return i;
    }
    if (alloc && NatsInCursum(sum) < static_cast<int>(kNatJournalEntries))
      return UpdateNatsInCursum(sum, 1);
  } else if (type == JournalType::kSitJournal) {
    for (int i = 0; i < SitsInCursum(sum); ++i) {
      if (LeToCpu(SegnoInJournal(sum, i)) == val)
        return i;
    }
    if (alloc && SitsInCursum(sum) < static_cast<int>(kSitJournalEntries))
      return UpdateSitsInCursum(sum, 1);
  }
  return -1;
}

SegmentManager::SegmentManager(F2fs *fs) : fs_(fs), superblock_info_(fs->GetSuperblockInfo()) {}
SegmentManager::SegmentManager(SuperblockInfo &info) : superblock_info_(info) {}

const SegmentEntry &SegmentManager::GetSegmentEntry(uint32_t segno) {
  fs::SharedLock sentry_lock(sentry_lock_);
  ZX_ASSERT(IsValidSegmentNumber(segno));
  return sit_info_->sentries[segno];
}

bool SegmentManager::CompareValidBlocks(uint32_t blocks, uint32_t segno, bool section) {
  fs::SharedLock lock(sentry_lock_);
  return GetValidBlocks(segno, section) == blocks;
}

uint32_t SegmentManager::GetValidBlocks(uint32_t segno, bool section) const {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  // In order to get # of valid blocks in a section instantly from many
  // segments, f2fs manages two counting structures separately.
  if (section && superblock_info_.GetSegsPerSec() > 1) {
    return sit_info_->sec_entries[GetSecNo(segno)].valid_blocks;
  }
  return sit_info_->sentries[segno].valid_blocks;
}

uint32_t SegmentManager::FindNextInuse(uint32_t max, uint32_t start) {
  fs::SharedLock segmap_lock(segmap_lock_);
  size_t segno = start;
  if (free_info_->free_segmap.Scan(segno, max, false, &segno)) {
    return max;
  }
  return safemath::checked_cast<uint32_t>(segno);
}

void SegmentManager::SetFree(uint32_t segno) {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  uint32_t secno = segno / superblock_info_.GetSegsPerSec();
  uint32_t start_segno = secno * superblock_info_.GetSegsPerSec();

  std::lock_guard segmap_lock(segmap_lock_);
  free_info_->free_segmap.ClearOne(segno);
  ++free_info_->free_segments;

  size_t dirty_seg;
  if (!free_info_->free_segmap.Scan(start_segno, TotalSegs(), false, &dirty_seg) &&
      dirty_seg < start_segno + superblock_info_.GetSegsPerSec()) {
    return;
  }
  free_info_->free_secmap.ClearOne(secno);
  ++free_info_->free_sections;
}

void SegmentManager::SetInuse(uint32_t segno) {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  uint32_t secno = segno / superblock_info_.GetSegsPerSec();
  free_info_->free_segmap.SetOne(segno);
  --free_info_->free_segments;
  if (!free_info_->free_secmap.GetOne(secno)) {
    free_info_->free_secmap.SetOne(secno);
    --free_info_->free_sections;
  }
}

void SegmentManager::SetTestAndFree(uint32_t segno) {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  uint32_t secno = segno / superblock_info_.GetSegsPerSec();
  uint32_t start_segno = secno * superblock_info_.GetSegsPerSec();
  std::lock_guard segmap_lock(segmap_lock_);
  if (free_info_->free_segmap.GetOne(segno)) {
    free_info_->free_segmap.ClearOne(segno);
    ++free_info_->free_segments;

    size_t next;
    if (!free_info_->free_segmap.Scan(start_segno, TotalSegs(), false, &next)) {
      if (next < start_segno + superblock_info_.GetSegsPerSec())
        return;
    }
    if (free_info_->free_secmap.GetOne(secno)) {
      free_info_->free_secmap.ClearOne(secno);
      ++free_info_->free_sections;
    }
  }
}

void SegmentManager::SetTestAndInuse(uint32_t segno) {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  uint32_t secno = segno / superblock_info_.GetSegsPerSec();
  std::lock_guard segmap_lock(segmap_lock_);
  if (!free_info_->free_segmap.GetOne(segno)) {
    free_info_->free_segmap.SetOne(segno);
    --free_info_->free_segments;
    if (!free_info_->free_secmap.GetOne(secno)) {
      free_info_->free_secmap.SetOne(secno);
      --free_info_->free_sections;
    }
  }
}

void SegmentManager::GetSitBitmap(void *dst_addr) {
  fs::SharedLock lock(sentry_lock_);
  CloneBits(dst_addr, sit_info_->sit_bitmap, 0, GetBitSize(sit_info_->bitmap_size));
}

block_t SegmentManager::FreeSegments() {
  fs::SharedLock segmap_lock(segmap_lock_);
  return free_info_->free_segments;
}

block_t SegmentManager::GetSectionCountForCheckpoint() {
  uint32_t pages_per_sec =
      safemath::CheckMul<uint32_t>(kDefaultBlocksPerSegment, superblock_info_.GetSegsPerSec())
          .ValueOrDie();
  uint32_t node_secs = CheckedDivRoundUp<uint32_t>(
      superblock_info_.GetPageCount(CountType::kDirtyNodes), pages_per_sec);
  uint32_t dent_secs = CheckedDivRoundUp<uint32_t>(
      superblock_info_.GetPageCount(CountType::kDirtyDents), pages_per_sec);
  return (node_secs + safemath::CheckMul<uint32_t>(dent_secs, 2)).ValueOrDie();
}

block_t SegmentManager::FreeSections() {
  fs::SharedLock segmap_lock(segmap_lock_);
  return free_info_->free_sections;
}

block_t SegmentManager::PrefreeSegments() {
  fs::SharedLock seglist_lock(seglist_lock_);
  return dirty_info_->nr_dirty[static_cast<int>(DirtyType::kPre)];
}

block_t SegmentManager::DirtySegments() {
  fs::SharedLock seglist_lock(seglist_lock_);
  return dirty_info_->nr_dirty[static_cast<int>(DirtyType::kDirtyHotData)] +
         dirty_info_->nr_dirty[static_cast<int>(DirtyType::kDirtyWarmData)] +
         dirty_info_->nr_dirty[static_cast<int>(DirtyType::kDirtyColdData)] +
         dirty_info_->nr_dirty[static_cast<int>(DirtyType::kDirtyHotNode)] +
         dirty_info_->nr_dirty[static_cast<int>(DirtyType::kDirtyWarmNode)] +
         dirty_info_->nr_dirty[static_cast<int>(DirtyType::kDirtyColdNode)];
}

block_t SegmentManager::ReservedSections() {
  block_t reserved_sections = reserved_segments_ / superblock_info_.GetSegsPerSec();
  // We don't need to reserve free sections for the next section of each temperature in ssr mode.
  if (!superblock_info_.TestOpt(MountOption::kForceLfs)) {
    return safemath::CheckSub<block_t>(reserved_sections, kNrCursegType).ValueOrDie();
  }
  return reserved_sections;
}
bool SegmentManager::NeedSSR() { return !superblock_info_.TestOpt(MountOption::kForceLfs); }

zx::result<> SegmentManager::GetSsrSegment(CursegType type) {
  CursegInfo *curseg = CURSEG_I(type);
  if (zx::result segno = GetVictimByDefault(GcType::kBgGc, type, AllocType::kSSR); segno.is_ok()) {
    curseg->next_segno = segno.value();
    return zx::ok();
  }

  // It failed to get a segment from the dirty list of |type|. Try to find a segment from dirty
  // lists of other types.
  std::span<const CursegType> types;
  if (IsNodeSeg(type)) {
    if (type == CursegType::kCursegHotNode) {
      static constexpr CursegType kTypes[] = {CursegType::kCursegWarmNode,
                                              CursegType::kCursegColdNode};
      types = kTypes;
    } else {
      static constexpr CursegType kTypes[] = {
          CursegType::kCursegColdNode, CursegType::kCursegWarmNode, CursegType::kCursegHotNode};
      types = kTypes;
    }
  } else {
    if (type == CursegType::kCursegHotData) {
      static constexpr CursegType kTypes[] = {CursegType::kCursegWarmData,
                                              CursegType::kCursegColdData};
      types = kTypes;
    } else {
      static constexpr CursegType kTypes[] = {
          CursegType::kCursegColdData, CursegType::kCursegWarmData, CursegType::kCursegHotData};
      types = kTypes;
    }
  }

  for (CursegType current : types) {
    if (current == type) {
      continue;
    }
    if (zx::result segno = GetVictimByDefault(GcType::kBgGc, current, AllocType::kSSR);
        segno.is_ok()) {
      curseg->next_segno = segno.value();
      return zx::ok();
    }
  }
  return zx::error(ZX_ERR_UNAVAILABLE);
}

uint32_t SegmentManager::Utilization() { return superblock_info_.Utilization(); }

// Sometimes f2fs may be better to drop out-of-place update policy.
// So, if fs utilization is over kMinIpuUtil, then f2fs tries to write
// data in the original place likewise other traditional file systems.
// TODO: Currently, we do not use IPU. Consider using IPU for fsynced data.
constexpr uint32_t kMinIpuUtil = 100;
bool SegmentManager::NeedInplaceUpdate(bool is_dir) {
  if (is_dir || superblock_info_.TestOpt(MountOption::kForceLfs)) {
    return false;
  }
  return NeedSSR() && Utilization() > kMinIpuUtil;
}

#if 0  // porting needed
// This function is used for only debugging.
// NOTE: In future, we have to remove this function.
void SegmentManager::VerifyBlockAddr(block_t blk_addr) {
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();
  SmInfo *sm_info = GetSmInfo(&superblock_info);
  block_t total_blks = sm_info->segment_count << superblock_info.GetLogBlocksPerSeg();
  [[maybe_unused]] block_t start_addr = sm_info->seg0_blkaddr;
  [[maybe_unused]] block_t end_addr = start_addr + total_blks - 1;
  ZX_ASSERT(!(blk_addr < start_addr));
  ZX_ASSERT(!(blk_addr > end_addr));
}
#endif

// Summary block is always treated as invalid block
void SegmentManager::CheckBlockCount(uint32_t segno, SitEntry &raw_sit) {
  int valid_blocks = 0;

  // check segment usage
  ZX_ASSERT(!(GetSitVblocks(raw_sit) > superblock_info_.GetBlocksPerSeg()));

  // check segment type
  ZX_ASSERT(IsValidSegmentType(GetSitType(raw_sit)));

  // check boundary of a given segment number
  ZX_ASSERT(IsValidSegmentNumber(segno));

  // check bitmap with valid block count
  PageBitmap bits(raw_sit.valid_map, kSitVBlockMapSizeInBit);
  for (uint32_t i = 0; i < superblock_info_.GetBlocksPerSeg(); ++i) {
    if (bits.Test(ToMsbFirst(i)))
      ++valid_blocks;
  }
  ZX_ASSERT(GetSitVblocks(raw_sit) == valid_blocks);
}

pgoff_t SegmentManager::CurrentSitAddr(uint32_t start) {
  uint32_t offset = SitBlockOffset(start);
  block_t blk_addr = sit_info_->sit_base_addr + offset;

  ZX_ASSERT(IsValidSegmentNumber(start));

  // calculate sit block address
  if (sit_info_->sit_bitmap.GetOne(ToMsbFirst(offset)))
    blk_addr += sit_info_->sit_blocks;

  return blk_addr;
}

pgoff_t SegmentManager::NextSitAddr(pgoff_t block_addr) {
  block_addr -= sit_info_->sit_base_addr;
  if (block_addr < sit_info_->sit_blocks)
    block_addr += sit_info_->sit_blocks;
  else
    block_addr -= sit_info_->sit_blocks;

  return block_addr + sit_info_->sit_base_addr;
}

void SegmentManager::SetToNextSit(uint32_t start) {
  size_t offset = ToMsbFirst(SitBlockOffset(start));
  if (sit_info_->sit_bitmap.GetOne(offset)) {
    sit_info_->sit_bitmap.ClearOne(offset);
  } else {
    sit_info_->sit_bitmap.SetOne(offset);
  }
}

uint64_t SegmentManager::GetMtime() const {
  auto cur_time = time(nullptr);
  return sit_info_->elapsed_time + cur_time - sit_info_->mounted_time;
}

block_t SegmentManager::StartSumBlock() const {
  return superblock_info_.StartCpAddr() +
         LeToCpu(superblock_info_.GetCheckpoint().cp_pack_start_sum);
}

block_t SegmentManager::SumBlkAddr(int base, int type) const {
  uint32_t total_blocks = LeToCpu(superblock_info_.GetCheckpoint().cp_pack_total_block_count);
  ZX_ASSERT(total_blocks >= static_cast<uint32_t>(base + 1));
  return superblock_info_.StartCpAddr() + total_blocks - (base + 1) + type;
}

bool SegmentManager::SecUsageCheck(unsigned int secno) const {
  return IsCurSec(secno) || cur_victim_sec_ == secno;
}

zx::result<> SegmentManager::HasEnoughSsrBlocks(size_t needed_blocks) {
  static constexpr CursegType kNodeTypes[] = {
      CursegType::kCursegColdNode, CursegType::kCursegWarmNode, CursegType::kCursegHotNode};
  static constexpr CursegType kDataTypes[] = {
      CursegType::kCursegHotData, CursegType::kCursegWarmData, CursegType::kCursegColdData};
  size_t free_segments = FreeSegments();
  size_t reserved_segments = ReservedSections() * superblock_info_.GetSegsPerSec();
  if (superblock_info_.TestOpt(MountOption::kForceLfs) || free_segments < reserved_segments) {
    return zx::error(ZX_ERR_NO_SPACE);
  }
  free_segments -= reserved_segments;

  size_t dirty_dir_blocks = superblock_info_.GetPageCount(CountType::kDirtyDents);
  size_t dirty_node_blocks = superblock_info_.GetPageCount(CountType::kDirtyNodes);
  size_t additional_needed = 0;
  fs::SharedLock lock(sentry_lock_);
  for (CursegType type : kNodeTypes) {
    size_t available = GetAvailableBlockCountOnCurseg(type);
    size_t needed = type == CursegType::kCursegHotNode
                        ? dirty_dir_blocks + dirty_node_blocks + needed_blocks
                        : dirty_node_blocks + needed_blocks;
    if (available < needed) {
      additional_needed = std::max(additional_needed, needed - available);
    }
  }

  constexpr bool kIsNode = true;
  size_t available_blocks = ckpt_invalid_blocks_[kIsNode];

  // Return zx::ok if there is enough space to commit checkpoint in SSR mode after |needed_blocks|
  // of dirty data pages.
  if (available_blocks < additional_needed) {
    size_t needed = CheckedDivRoundUp(additional_needed - available_blocks,
                                      static_cast<size_t>(kDefaultBlocksPerSegment));
    if (free_segments < needed) {
      return zx::error(ZX_ERR_NO_SPACE);
    }
    free_segments -= needed;
  }

  additional_needed = 0;
  available_blocks = ckpt_invalid_blocks_[!kIsNode] +
                     free_segments * static_cast<size_t>(kDefaultBlocksPerSegment);
  for (CursegType type : kDataTypes) {
    size_t available = GetAvailableBlockCountOnCurseg(type);
    size_t needed =
        type == CursegType::kCursegHotData ? needed_blocks + dirty_dir_blocks : needed_blocks;
    if (available < needed) {
      additional_needed = std::max(additional_needed, needed - available);
    }
  }

  if (additional_needed > available_blocks) {
    return zx::error(ZX_ERR_NO_SPACE);
  }
  return zx::ok();
}

bool SegmentManager::HasNotEnoughFreeSecs(size_t freed_sections, size_t needed_blocks) {
  if (fs_->IsOnRecovery())
    return false;

  size_t blocks_per_section = safemath::CheckMul<uint32_t>(superblock_info_.GetBlocksPerSeg(),
                                                           superblock_info_.GetSegsPerSec())
                                  .ValueOrDie();
  size_t needed_sections = CheckedDivRoundUp(needed_blocks, blocks_per_section) +
                           GetSectionCountForCheckpoint() + ReservedSections();
  return FreeSections() + freed_sections <= needed_sections;
}

void SegmentManager::LocateDirtySegment(uint32_t segno, DirtyType dirty_type) {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  // The current segments should not belong to dirty_segmap.
  if (IsCurSeg(segno)) {
    return;
  }

  if (!dirty_info_->dirty_segmap[static_cast<int>(dirty_type)].GetOne(segno)) {
    dirty_info_->dirty_segmap[static_cast<int>(dirty_type)].SetOne(segno);
    ++dirty_info_->nr_dirty[static_cast<int>(dirty_type)];
    if (dirty_type == DirtyType::kDirty) {
      SegmentEntry &segment_entry = sit_info_->sentries[segno];
      UpdateInvalidBlocks(segment_entry.ckpt_invalid_blocks,
                          IsNodeSeg(static_cast<CursegType>(segment_entry.type)));
    }
  }

  if (dirty_type == DirtyType::kDirty) {
    uint8_t entry_type = sit_info_->sentries[segno].type;
    ZX_ASSERT(IsValidSegmentType(entry_type));
    if (!dirty_info_->dirty_segmap[entry_type].GetOne(segno)) {
      dirty_info_->dirty_segmap[entry_type].SetOne(segno);
      ++dirty_info_->nr_dirty[entry_type];
    }
  }
}

void SegmentManager::RemoveDirtySegment(uint32_t segno, DirtyType dirty_type) {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  if (dirty_info_->dirty_segmap[static_cast<int>(dirty_type)].GetOne(segno)) {
    dirty_info_->dirty_segmap[static_cast<int>(dirty_type)].ClearOne(segno);
    --dirty_info_->nr_dirty[static_cast<int>(dirty_type)];
    if (dirty_type == DirtyType::kDirty) {
      SegmentEntry &segment_entry = sit_info_->sentries[segno];
      UpdateInvalidBlocks(-segment_entry.ckpt_invalid_blocks,
                          IsNodeSeg(static_cast<CursegType>(segment_entry.type)));
    }
  }

  if (dirty_type == DirtyType::kDirty) {
    uint8_t entry_type = sit_info_->sentries[segno].type;
    ZX_ASSERT(IsValidSegmentType(entry_type));
    if (dirty_info_->dirty_segmap[entry_type].GetOne(segno)) {
      dirty_info_->dirty_segmap[entry_type].ClearOne(segno);
      --dirty_info_->nr_dirty[entry_type];
    }
    if (GetValidBlocks(segno, true) == 0) {
      dirty_info_->victim_secmap.ClearOne(GetSecNo(segno));
    }
  }
}

// Should not occur error such as ZX_ERR_NO_MEMORY.
// Adding dirty entry into seglist is not critical operation.
// If a given segment is one of current working segments, it won't be added.
void SegmentManager::LocateDirtySegment(uint32_t segno) {
  if (segno == kNullSegNo || IsCurSeg(segno))
    return;

  ZX_ASSERT(IsValidSegmentNumber(segno));

  std::lock_guard seglist_lock(seglist_lock_);

  uint32_t valid_blocks = GetValidBlocks(segno, false);

  if (valid_blocks == 0) {
    LocateDirtySegment(segno, DirtyType::kPre);
    RemoveDirtySegment(segno, DirtyType::kDirty);
  } else if (valid_blocks < superblock_info_.GetBlocksPerSeg()) {
    LocateDirtySegment(segno, DirtyType::kDirty);
  } else {
    // Recovery routine with SSR needs this
    RemoveDirtySegment(segno, DirtyType::kDirty);
  }
}

// Should call clear_prefree_segments after checkpoint is done.
void SegmentManager::SetPrefreeAsFreeSegments() {
  std::lock_guard seglist_lock(seglist_lock_);
  const size_t total_segs = TotalSegs();
  for (size_t segno = 0; !dirty_info_->dirty_segmap[static_cast<int>(DirtyType::kPre)].Scan(
           segno, total_segs, false, &segno);
       ++segno) {
    SetTestAndFree(safemath::checked_cast<uint32_t>(segno));
  }
}

void SegmentManager::ClearPrefreeSegments() {
  const uint32_t total_segs = TotalSegs();
  bool need_align =
      superblock_info_.TestOpt(MountOption::kForceLfs) && superblock_info_.GetSegsPerSec() > 1;

  std::lock_guard seglist_lock(seglist_lock_);
  for (size_t segno = 0; !dirty_info_->dirty_segmap[static_cast<int>(DirtyType::kPre)].Scan(
           segno, total_segs, false, &segno);) {
    uint32_t start = safemath::checked_cast<uint32_t>(segno);
    uint32_t end;
    if (need_align) {
      start = GetSecNo(start) * superblock_info_.GetSegsPerSec();
      end = start + superblock_info_.GetSegsPerSec();
    } else {
      if (dirty_info_->dirty_segmap[static_cast<int>(DirtyType::kPre)].Scan(start + 1, total_segs,
                                                                            true, &segno)) {
        segno = total_segs;
      }
      end = safemath::checked_cast<uint32_t>(segno);
    }

    for (size_t i = start; i < end; ++i) {
      if (dirty_info_->dirty_segmap[static_cast<int>(DirtyType::kPre)].GetOne(i)) {
        dirty_info_->dirty_segmap[static_cast<int>(DirtyType::kPre)].ClearOne(i);
        --dirty_info_->nr_dirty[static_cast<int>(DirtyType::kPre)];
      }
    }
    segno = end;

    if (!superblock_info_.TestOpt(MountOption::kDiscard)) {
      continue;
    }

    if (!need_align) {
      block_t num_of_blocks =
          (safemath::CheckSub<block_t>(end, start) * superblock_info_.GetBlocksPerSeg())
              .ValueOrDie();
      fs_->MakeTrimOperation(StartBlock(start), num_of_blocks);
    } else {
      // In kMountForceLfs mode, a section is reusable only when all
      // segments of the section are free. Therefore, trim operation is
      // performed in section unit only in this case.
      while (start < end) {
        uint32_t secno = GetSecNo(start);
        uint32_t start_segno =
            safemath::CheckMul(secno, superblock_info_.GetSegsPerSec()).ValueOrDie();
        if (!IsCurSec(secno) && CompareValidBlocks(0, start, true)) {
          block_t num_of_blocks = safemath::CheckMul<block_t>(superblock_info_.GetSegsPerSec(),
                                                              superblock_info_.GetBlocksPerSeg())
                                      .ValueOrDie();
          fs_->MakeTrimOperation(StartBlock(start_segno), num_of_blocks);
        }
        start = safemath::CheckAdd(start_segno, superblock_info_.GetSegsPerSec()).ValueOrDie();
      }
    }
  }
}

void SegmentManager::MarkSitEntryDirty(uint32_t segno) {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  if (!sit_info_->dirty_sentries_bitmap.GetOne(segno)) {
    sit_info_->dirty_sentries_bitmap.SetOne(segno);
    ++sit_info_->dirty_sentries;
  }
}

void SegmentManager::SetSitEntryType(CursegType type, uint32_t segno, bool modified) {
  ZX_ASSERT(IsValidSegmentNumber(segno));
  ZX_ASSERT(IsValidSegmentType(type));
  SegmentEntry &segment_entry = sit_info_->sentries[segno];
  segment_entry.type = static_cast<uint8_t>(type);
  if (modified)
    MarkSitEntryDirty(segno);
}

void SegmentManager::UpdateSitEntry(block_t blkaddr, int del) {
  uint32_t offset;
  uint64_t new_vblocks;
  uint32_t segno = GetSegmentNumber(blkaddr);
  ZX_ASSERT(IsValidSegmentNumber(segno));
  SegmentEntry &segment_entry = sit_info_->sentries[segno];

  new_vblocks = segment_entry.valid_blocks + del;
  offset = GetSegOffFromSeg0(blkaddr) & (superblock_info_.GetBlocksPerSeg() - 1);

  ZX_ASSERT(!((new_vblocks >> (sizeof(uint16_t) << 3) ||
               (new_vblocks > superblock_info_.GetBlocksPerSeg()))));

  segment_entry.valid_blocks = static_cast<uint16_t>(new_vblocks);
  segment_entry.mtime = GetMtime();
  sit_info_->max_mtime = segment_entry.mtime;

  // Update valid block bitmap
  if (del > 0) {
    ZX_ASSERT(!segment_entry.cur_valid_map.GetOne(ToMsbFirst(offset)));
    segment_entry.cur_valid_map.SetOne(ToMsbFirst(offset));
    // We do not allow SSR overwrite. If a fsyncd data block is invalidated and rewritten, it can
    // cause data corruption when spor happens before checkpoint
    if (!segment_entry.ckpt_valid_map.GetOne(ToMsbFirst(offset))) {
      segment_entry.ckpt_valid_map.SetOne(ToMsbFirst(offset));
      ++segment_entry.ckpt_valid_blocks;
      --segment_entry.ckpt_invalid_blocks;
      // No need to call UpdateInvalidBlocks() as curseg cannot belong to dirty_segmap.
    }
  } else {
    ZX_ASSERT(segment_entry.cur_valid_map.GetOne(ToMsbFirst(offset)));
    segment_entry.cur_valid_map.ClearOne(ToMsbFirst(offset));
  }
  if (!segment_entry.ckpt_valid_map.GetOne(ToMsbFirst(offset))) {
    segment_entry.ckpt_valid_blocks += del;
    segment_entry.ckpt_invalid_blocks -= del;
    fs::SharedLock seglist_lock(seglist_lock_);
    if (dirty_info_->dirty_segmap[static_cast<int>(DirtyType::kDirty)].GetOne(segno)) {
      UpdateInvalidBlocks(-del, IsNodeSeg(static_cast<CursegType>(segment_entry.type)));
    }
  }

  MarkSitEntryDirty(segno);

  // update total number of valid blocks to be written in ckpt area
  sit_info_->written_valid_blocks += del;

  if (superblock_info_.GetSegsPerSec() > 1)
    sit_info_->sec_entries[GetSecNo(segno)].valid_blocks += del;
}

void SegmentManager::RefreshSitEntry(block_t old_blkaddr, block_t new_blkaddr) {
  UpdateSitEntry(new_blkaddr, 1);
  if (GetSegmentNumber(old_blkaddr) != kNullSegNo)
    UpdateSitEntry(old_blkaddr, -1);
}

void SegmentManager::InvalidateBlocks(block_t addr) {
  ZX_ASSERT(addr != kNullAddr);
  if (addr == kNewAddr) {
    return;
  }

  std::lock_guard sentry_lock(sentry_lock_);

  // add it into sit main buffer
  UpdateSitEntry(addr, -1);

  // add it into dirty seglist
  LocateDirtySegment(GetSegmentNumber(addr));
}

// This function should be resided under the curseg_mutex lock
void SegmentManager::AddSumEntry(CursegType type, Summary *sum, uint16_t offset) {
  CursegInfo *curseg = CURSEG_I(type);
  curseg->sum_blk->entries[offset] = *sum;
}

// Calculate the number of current summary pages for writing
int SegmentManager::NpagesForSummaryFlush() {
  uint32_t total_size_bytes = 0;
  uint32_t valid_sum_count = 0;

  for (CursegType type :
       {CursegType::kCursegHotData, CursegType::kCursegWarmData, CursegType::kCursegColdData}) {
    if (CursegAllocType(type) == AllocType::kSSR) {
      valid_sum_count += superblock_info_.GetBlocksPerSeg();
    } else {
      valid_sum_count += CursegBlkoff(type);
    }
  }

  total_size_bytes =
      (safemath::CheckMul<uint32_t>(valid_sum_count, kSummarySize + 1) +
       safemath::checked_cast<uint32_t>(sizeof(NatJournal) + 2U + sizeof(SitJournal) + 2U))
          .ValueOrDie();
  uint32_t sum_space = kBlockSize - kSumFooterSize;
  if (total_size_bytes < sum_space) {
    return 1;
  } else if (total_size_bytes < 2 * sum_space) {
    return 2;
  }
  return 3;
}

// Caller should put this summary page
zx_status_t SegmentManager::GetSumPage(uint32_t segno, LockedPage *out) {
  return fs_->GetMetaPage(GetSumBlock(segno), out);
}

void SegmentManager::WriteSumPage(SummaryBlock *sum_blk, block_t blk_addr) {
  LockedPage page;
  fs_->GrabMetaPage(blk_addr, &page);
  page->Write(sum_blk);
  page.SetDirty();
}

// Find a new segment from the free segments bitmap to right order
// This function should be returned with success, otherwise BUG
// TODO: after LFS allocation available, raise out of space event of inspect tree when new segment
// cannot be allocated.
void SegmentManager::GetNewSegment(uint32_t *newseg, bool new_sec) {
  uint32_t total_secs = superblock_info_.GetTotalSections();
  uint32_t segno, secno, zoneno;
  uint32_t total_zones = superblock_info_.GetTotalSections() / superblock_info_.GetSecsPerZone();
  uint32_t hint = *newseg / superblock_info_.GetSegsPerSec();
  uint32_t old_zoneno = GetZoneNoFromSegNo(*newseg);
  bool find_another = true;
  bool got_it = false;

  std::lock_guard segmap_lock(segmap_lock_);

  if (!new_sec && ((*newseg + 1) % superblock_info_.GetSegsPerSec())) {
    size_t free_seg;
    if (free_info_->free_segmap.Scan(*newseg + 1, TotalSegs(), true, &free_seg)) {
      free_seg = TotalSegs();
    } else {
      got_it = true;
    }
    segno = safemath::checked_cast<uint32_t>(free_seg);
  }

  while (!got_it) {
    size_t free_sec;
    if (free_info_->free_secmap.Scan(hint, total_secs, true, &free_sec)) {
      ZX_ASSERT(!free_info_->free_secmap.Scan(0, total_secs, true, &free_sec));
    }
    secno = safemath::checked_cast<uint32_t>(free_sec);
    hint = secno;
    segno = secno * superblock_info_.GetSegsPerSec();
    zoneno = secno / superblock_info_.GetSecsPerZone();

    // give up on finding another zone
    if (!find_another) {
      break;
    }
    if (superblock_info_.GetSecsPerZone() == 1) {
      break;
    }
    if (zoneno == old_zoneno) {
      break;
    }

    bool is_current_zone = false;
    for (size_t i = 0; i < kNrCursegType; ++i) {
      if (CURSEG_I(static_cast<CursegType>(i))->zone == zoneno) {
        is_current_zone = true;
        break;
      }
    }
    if (!is_current_zone) {
      break;
    }
    // zone is in use, try another
    if (zoneno + 1 >= total_zones) {
      hint = 0;
    } else {
      hint = (zoneno + 1) * superblock_info_.GetSecsPerZone();
    }
    find_another = false;
  }
  // set it as dirty segment in free segmap
  ZX_ASSERT(!free_info_->free_segmap.GetOne(segno));
  SetInuse(segno);
  *newseg = segno;
}

void SegmentManager::ResetCurseg(CursegType type, bool modified) {
  CursegInfo *curseg = CURSEG_I(type);
  SummaryFooter *sum_footer;

  curseg->segno = curseg->next_segno;
  curseg->zone = GetZoneNoFromSegNo(curseg->segno);
  curseg->next_blkoff = 0;
  curseg->next_segno = kNullSegNo;

  sum_footer = &(curseg->sum_blk->footer);
  memset(sum_footer, 0, sizeof(SummaryFooter));
  if (IsDataSeg(type))
    SetSumType(sum_footer, kSumTypeData);
  if (IsNodeSeg(type))
    SetSumType(sum_footer, kSumTypeNode);
  SetSitEntryType(type, curseg->segno, modified);
}

// Allocate a current working segment.
// This function always allocates a free segment in LFS manner.
void SegmentManager::NewCurseg(CursegType type, bool new_sec) {
  CursegInfo *curseg = CURSEG_I(type);
  uint32_t segno = curseg->segno;
  WriteSumPage(&curseg->sum_blk, GetSumBlock(curseg->segno));
  GetNewSegment(&segno, new_sec);
  curseg->next_segno = segno;
  ResetCurseg(type, 1);
  curseg->alloc_type = AllocType::kLFS;
}

void SegmentManager::NextFreeBlkoff(CursegInfo *seg, block_t start) {
  SegmentEntry &segment_entry = sit_info_->sentries[seg->segno];
  block_t ofs;
  for (ofs = start; ofs < superblock_info_.GetBlocksPerSeg(); ++ofs) {
    if (!segment_entry.ckpt_valid_map.GetOne(ToMsbFirst(ofs)) &&
        !segment_entry.cur_valid_map.GetOne(ToMsbFirst(ofs)))
      break;
  }
  seg->next_blkoff = static_cast<uint16_t>(ofs);
}

// If a segment is written by LFS manner, next block offset is just obtained
// by increasing the current block offset. However, if a segment is written by
// SSR manner, next block offset obtained by calling __next_free_blkoff
void SegmentManager::RefreshNextBlkoff(CursegInfo *seg) {
  if (seg->alloc_type == AllocType::kSSR) {
    NextFreeBlkoff(seg, seg->next_blkoff + 1);
  } else {
    ++seg->next_blkoff;
  }
}

// This function always allocates a used segment (from dirty seglist) by SSR
// manner, so it should recover the existing segment information of valid blocks
void SegmentManager::ChangeCurseg(CursegType type, bool reuse) {
  CursegInfo *curseg = CURSEG_I(type);
  uint32_t new_segno = curseg->next_segno;

  WriteSumPage(&curseg->sum_blk, GetSumBlock(curseg->segno));
  SetTestAndInuse(new_segno);

  {
    std::lock_guard seglist_lock(seglist_lock_);
    RemoveDirtySegment(new_segno, DirtyType::kPre);
    RemoveDirtySegment(new_segno, DirtyType::kDirty);
  }

  ResetCurseg(type, 1);
  curseg->alloc_type = AllocType::kSSR;
  NextFreeBlkoff(curseg, 0);

  if (reuse) {
    LockedPage sum_page;
    ZX_ASSERT(GetSumPage(new_segno, &sum_page) == ZX_OK);
    sum_page->Read(curseg->sum_blk.get(), 0, kSumEntrySize);
  }
}

// Allocate a segment for new block allocations in CURSEG_I(type).
// This function is supposed to be successful. Otherwise, BUG.
void SegmentManager::AllocateSegmentByDefault(CursegType type, bool force) {
  CursegInfo *curseg = CURSEG_I(type);

  if (force) {
    NewCurseg(type, true);
  } else if (!superblock_info_.TestCpFlags(CpFlag::kCpCrcRecoveryFlag) &&
             type == CursegType::kCursegWarmNode) {
    NewCurseg(type, false);
  } else if (NeedSSR() && GetSsrSegment(type).is_ok()) {
    ChangeCurseg(type, true);
  } else {
    NewCurseg(type, false);
  }
  superblock_info_.IncSegmentCount(curseg->alloc_type);
}

void SegmentManager::AllocateNewSegments() {
  std::lock_guard sentry_lock(sentry_lock_);
  for (int i = static_cast<int>(CursegType::kCursegHotData);
       i <= static_cast<int>(CursegType::kCursegColdData); ++i) {
    CursegInfo *curseg = CURSEG_I(static_cast<CursegType>(i));
    uint32_t old_curseg = curseg->segno;
    AllocateSegmentByDefault(static_cast<CursegType>(i), true);
    LocateDirtySegment(old_curseg);
  }
}

bool SegmentManager::HasCursegSpace(CursegType type) {
  CursegInfo *curseg = CURSEG_I(type);
  return curseg->next_blkoff < superblock_info_.GetBlocksPerSeg();
}

block_t SegmentManager::GetBlockAddrOnSegment(LockedPage &page, block_t old_blkaddr, Summary *sum,
                                              PageType p_type) {
  CursegInfo *curseg;
  CursegType type;

  type = GetSegmentType(*page, p_type, superblock_info_.GetActiveLogs());
  curseg = CURSEG_I(type);

  block_t new_blkaddr;
  {
    std::lock_guard curseg_lock(curseg->curseg_mutex);
    new_blkaddr = NextFreeBlkAddr(type);

    // AddSumEntry should be resided under the curseg_mutex
    // because this function updates a summary entry in the
    // current summary block.
    AddSumEntry(type, sum, curseg->next_blkoff);

    {
      std::lock_guard sentry_lock(sentry_lock_);
      RefreshNextBlkoff(curseg);
      superblock_info_.IncBlockCount(curseg->alloc_type);

      // SIT information should be updated before segment allocation,
      // since SSR needs latest valid block information.
      RefreshSitEntry(old_blkaddr, new_blkaddr);

      if (!HasCursegSpace(type)) {
        AllocateSegmentByDefault(type, false);
      }

      LocateDirtySegment(GetSegmentNumber(old_blkaddr));
      LocateDirtySegment(GetSegmentNumber(new_blkaddr));
    }

    if (p_type == PageType::kNode) {
      page.GetPage<NodePage>().FillNodeFooterBlkaddr(NextFreeBlkAddr(type),
                                                     superblock_info_.GetCheckpointVer(true));
    }
  }
  return new_blkaddr;
}

void SegmentManager::RecoverDataPage(Summary &sum, block_t old_blkaddr, block_t new_blkaddr) {
  if (!IsValidMainBlockAddress(new_blkaddr)) {
    return;
  }
  uint32_t segno = GetSegmentNumber(new_blkaddr);
  if (!IsValidSegmentNumber(segno)) {
    return;
  }
  SegmentEntry &segment_entry = sit_info_->sentries[segno];

  CursegType type = static_cast<CursegType>(segment_entry.type);
  if (!IsValidSegmentType(type)) {
    FX_LOGS(WARNING) << "Invalid segment type " << static_cast<uint32_t>(segment_entry.type)
                     << " during recovery for segno " << segno;
    return;
  }

  if (segment_entry.valid_blocks == 0 && !IsCurSeg(segno)) {
    if (old_blkaddr == kNullAddr || !IsValidMainBlockAddress(old_blkaddr)) {
      type = CursegType::kCursegColdData;
    } else {
      type = CursegType::kCursegWarmData;
    }
  }
  CursegInfo *curseg = CURSEG_I(type);

  std::lock_guard curseg_lock(curseg->curseg_mutex);
  std::lock_guard sentry_lock(sentry_lock_);

  uint32_t old_cursegno = curseg->segno;

  // change the current segment
  if (segno != curseg->segno) {
    curseg->next_segno = segno;
    ChangeCurseg(type, true);
  }

  curseg->next_blkoff = safemath::checked_cast<uint16_t>(GetSegOffFromSeg0(new_blkaddr) &
                                                         (superblock_info_.GetBlocksPerSeg() - 1));
  AddSumEntry(type, &sum, curseg->next_blkoff);

  RefreshSitEntry(old_blkaddr, new_blkaddr);

  LocateDirtySegment(old_cursegno);
  LocateDirtySegment(GetSegmentNumber(old_blkaddr));
  LocateDirtySegment(segno);
}

zx_status_t SegmentManager::ReadCompactedSummaries() {
  const Checkpoint &ckpt = superblock_info_.GetCheckpoint();
  LockedPage page;
  block_t start;
  int offset;

  start = StartSumBlock();

  if (zx_status_t ret = fs_->GetMetaPage(start++, &page); ret != ZX_OK) {
    return ret;
  }

  // Step 1: restore nat cache
  CursegInfo *seg_i = CURSEG_I(CursegType::kCursegHotData);
  page->Read(&seg_i->sum_blk->n_nats, 0, kSumJournalSize);
  if (NatsInCursum(*seg_i->sum_blk) > static_cast<int>(kNatJournalEntries)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // Step 2: restore sit cache
  seg_i = CURSEG_I(CursegType::kCursegColdData);
  page->Read(&seg_i->sum_blk->n_sits, kSumJournalSize, kSumJournalSize);
  if (SitsInCursum(*seg_i->sum_blk) > static_cast<int>(kSitJournalEntries)) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  offset = 2 * kSumJournalSize;

  // Step 3: restore summary entries
  for (int i = static_cast<int>(CursegType::kCursegHotData);
       i <= static_cast<int>(CursegType::kCursegColdData); ++i) {
    uint16_t blk_off;
    uint32_t segno;

    seg_i = CURSEG_I(static_cast<CursegType>(i));
    segno = LeToCpu(ckpt.cur_data_segno[i]);
    blk_off = LeToCpu(ckpt.cur_data_blkoff[i]);
    if (!IsValidSegmentNumber(segno) || blk_off >= superblock_info_.GetBlocksPerSeg()) {
      return ZX_ERR_INVALID_ARGS;
    }
    seg_i->next_segno = segno;
    ResetCurseg(static_cast<CursegType>(i), 0);
    seg_i->alloc_type = static_cast<AllocType>(ckpt.alloc_type[i]);
    seg_i->next_blkoff = blk_off;

    if (seg_i->alloc_type == AllocType::kSSR)
      blk_off = static_cast<uint16_t>(superblock_info_.GetBlocksPerSeg());

    for (int j = 0; j < blk_off; ++j) {
      page->Read(&seg_i->sum_blk->entries[j], offset, kSummarySize);
      offset += kSummarySize;
      if (offset + kSummarySize <= kBlockSize - kSumFooterSize) {
        continue;
      }

      if (zx_status_t ret = fs_->GetMetaPage(start++, &page); ret != ZX_OK) {
        return ret;
      }
      offset = 0;
    }
  }
  return ZX_OK;
}

zx_status_t SegmentManager::ReadNormalSummaries(int type) {
  const Checkpoint &ckpt = superblock_info_.GetCheckpoint();
  SummaryBlock *sum;
  CursegInfo *curseg;
  uint16_t blk_off;
  uint32_t segno = 0;
  block_t blk_addr = 0;

  // get segment number and block addr
  if (IsDataSeg(static_cast<CursegType>(type))) {
    segno = LeToCpu(ckpt.cur_data_segno[type]);
    blk_off = LeToCpu(ckpt.cur_data_blkoff[type - static_cast<int>(CursegType::kCursegHotData)]);
    if (!IsValidSegmentNumber(segno) || blk_off >= superblock_info_.GetBlocksPerSeg()) {
      return ZX_ERR_INVALID_ARGS;
    }
    if (superblock_info_.TestCpFlags(CpFlag::kCpUmountFlag)) {
      blk_addr = SumBlkAddr(kNrCursegType, type);
    } else {
      blk_addr = SumBlkAddr(kNrCursegDataType, type);
    }
  } else {
    segno = LeToCpu(ckpt.cur_node_segno[type - static_cast<int>(CursegType::kCursegHotNode)]);
    blk_off = LeToCpu(ckpt.cur_node_blkoff[type - static_cast<int>(CursegType::kCursegHotNode)]);
    if (!IsValidSegmentNumber(segno) || blk_off >= superblock_info_.GetBlocksPerSeg()) {
      return ZX_ERR_INVALID_ARGS;
    }
    if (superblock_info_.TestCpFlags(CpFlag::kCpUmountFlag)) {
      blk_addr = SumBlkAddr(kNrCursegNodeType, type - static_cast<int>(CursegType::kCursegHotNode));
    } else {
      blk_addr = GetSumBlock(segno);
    }
  }

  LockedPage new_page;
  if (zx_status_t ret = fs_->GetMetaPage(blk_addr, &new_page); ret != ZX_OK) {
    return ret;
  }
  sum = new_page->GetAddress<SummaryBlock>();

  if (IsNodeSeg(static_cast<CursegType>(type))) {
    if (superblock_info_.TestCpFlags(CpFlag::kCpUmountFlag)) {
      Summary *ns = &sum->entries[0];
      for (uint32_t i = 0; i < superblock_info_.GetBlocksPerSeg(); ++i, ++ns) {
        ns->version = 0;
        ns->ofs_in_node = 0;
      }
    } else {
      if (RestoreNodeSummary(segno, *sum)) {
        return ZX_ERR_INVALID_ARGS;
      }
    }
  }

  // set uncompleted segment to curseg
  curseg = CURSEG_I(static_cast<CursegType>(type));
  {
    std::lock_guard curseg_lock(curseg->curseg_mutex);
    new_page->Read(curseg->sum_blk.get());

    if (type == static_cast<int>(CursegType::kCursegHotData) &&
        NatsInCursum(*curseg->sum_blk) > static_cast<int>(kNatJournalEntries)) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    if (type == static_cast<int>(CursegType::kCursegColdData) &&
        SitsInCursum(*curseg->sum_blk) > static_cast<int>(kSitJournalEntries)) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    curseg->next_segno = segno;
    ResetCurseg(static_cast<CursegType>(type), 0);
    curseg->alloc_type = static_cast<AllocType>(ckpt.alloc_type[type]);
    curseg->next_blkoff = blk_off;
  }
  return ZX_OK;
}

zx_status_t SegmentManager::RestoreNodeSummary(uint32_t segno, SummaryBlock &sum) {
  uint32_t last_offset = superblock_info_.GetBlocksPerSeg();
  block_t addr = StartBlock(segno);
  Summary *sum_entry = &sum.entries[0];

  for (uint32_t i = 0; i < last_offset; ++i, ++sum_entry, ++addr) {
    LockedPage page;
    if (zx_status_t ret = fs_->GetMetaPage(addr, &page); ret != ZX_OK) {
      return ret;
    }

    sum_entry->nid = page->GetAddress<Node>()->footer.nid;
    sum_entry->version = 0;
    sum_entry->ofs_in_node = 0;
  }
  return ZX_OK;
}

zx_status_t SegmentManager::RestoreCursegSummaries() {
  int type = static_cast<int>(CursegType::kCursegHotData);

  std::lock_guard sentry_lock(sentry_lock_);
  if (superblock_info_.TestCpFlags(CpFlag::kCpCompactSumFlag)) {
    // restore for compacted data summary
    if (ReadCompactedSummaries())
      return ZX_ERR_INVALID_ARGS;
    type = static_cast<int>(CursegType::kCursegHotNode);
  }

  for (; type <= static_cast<int>(CursegType::kCursegColdNode); ++type) {
    if (ReadNormalSummaries(type))
      return ZX_ERR_INVALID_ARGS;
  }
  return ZX_OK;
}

void SegmentManager::WriteCompactedSummaries(block_t blkaddr) {
  LockedPage page;
  fs_->GrabMetaPage(blkaddr++, &page);

  size_t written_size = 0;
  // Step 1: write nat cache
  CursegInfo *seg_i = CURSEG_I(CursegType::kCursegHotData);
  page->Write(&seg_i->sum_blk->n_nats, 0, kSumJournalSize);
  written_size += kSumJournalSize;

  // Step 2: write sit cache
  seg_i = CURSEG_I(CursegType::kCursegColdData);
  page->Write(&seg_i->sum_blk->n_sits, written_size, kSumJournalSize);
  written_size += kSumJournalSize;

  page.SetDirty();

  // Step 3: write summary entries
  for (CursegType type :
       {CursegType::kCursegHotData, CursegType::kCursegWarmData, CursegType::kCursegColdData}) {
    size_t blkoff;
    seg_i = CURSEG_I(type);
    if (seg_i->alloc_type == AllocType::kSSR) {
      blkoff = superblock_info_.GetBlocksPerSeg();
    } else {
      blkoff = seg_i->next_blkoff;
    }

    for (size_t j = 0; j < blkoff; ++j) {
      if (!page) {
        fs_->GrabMetaPage(blkaddr++, &page);
        written_size = 0;
        page.SetDirty();
      }
      page->Write(&seg_i->sum_blk->entries[j], written_size, kSummarySize);
      written_size += kSummarySize;

      if (written_size + kSummarySize <= kBlockSize - kSumFooterSize) {
        continue;
      }

      page.reset();
    }
  }
}

void SegmentManager::WriteNormalSummaries(block_t blkaddr, CursegType type) {
  int end;

  if (IsDataSeg(type)) {
    end = static_cast<int>(type) + kNrCursegDataType;
  } else {
    end = static_cast<int>(type) + kNrCursegNodeType;
  }

  for (int i = static_cast<int>(type); i < end; ++i) {
    CursegInfo *sum = CURSEG_I(static_cast<CursegType>(i));
    std::lock_guard curseg_lock(sum->curseg_mutex);
    WriteSumPage(&sum->sum_blk, blkaddr + (i - static_cast<int>(type)));
  }
}

void SegmentManager::WriteDataSummaries(block_t start_blk) {
  if (superblock_info_.TestCpFlags(CpFlag::kCpCompactSumFlag)) {
    WriteCompactedSummaries(start_blk);
  } else {
    WriteNormalSummaries(start_blk, CursegType::kCursegHotData);
  }
}

void SegmentManager::WriteNodeSummaries(block_t start_blk) {
  if (superblock_info_.TestCpFlags(CpFlag::kCpUmountFlag))
    WriteNormalSummaries(start_blk, CursegType::kCursegHotNode);
}

zx::result<LockedPage> SegmentManager::GetCurrentSitPage(uint32_t segno) {
  uint32_t offset = SitBlockOffset(segno);
  block_t blk_addr = sit_info_->sit_base_addr + offset;

  ZX_ASSERT(IsValidSegmentNumber(segno));

  // calculate sit block address
  if (sit_info_->sit_bitmap.GetOne(ToMsbFirst(offset)))
    blk_addr += sit_info_->sit_blocks;

  LockedPage locked_page;
  if (zx_status_t ret = fs_->GetMetaPage(blk_addr, &locked_page); ret != ZX_OK) {
    return zx::error(ret);
  }
  return zx::ok(std::move(locked_page));
}

zx::result<LockedPage> SegmentManager::GetNextSitPage(uint32_t start) {
  pgoff_t src_off, dst_off;

  src_off = CurrentSitAddr(start);
  dst_off = NextSitAddr(src_off);

  // get current sit block page without lock
  LockedPage src_page;
  if (zx_status_t ret = fs_->GetMetaPage(src_off, &src_page); ret != ZX_OK) {
    return zx::error(ret);
  }
  LockedPage dst_page;
  fs_->GrabMetaPage(dst_off, &dst_page);
  ZX_ASSERT(!src_page->IsDirty());

  dst_page->Write(src_page->GetAddress());

  dst_page.SetDirty();

  SetToNextSit(start);

  return zx::ok(std::move(dst_page));
}

bool SegmentManager::FlushSitsInJournal() {
  CursegInfo *curseg = CURSEG_I(CursegType::kCursegColdData);
  SummaryBlock *sum = &curseg->sum_blk;

  // If the journal area in the current summary is full of sit entries,
  // all the sit entries will be flushed. Otherwise the sit entries
  // are not able to replace with newly hot sit entries.
  if ((SitsInCursum(*sum) + sit_info_->dirty_sentries) > static_cast<int>(kSitJournalEntries)) {
    for (int i = SitsInCursum(*sum) - 1; i >= 0; --i) {
      uint32_t segno;
      segno = LeToCpu(SegnoInJournal(*sum, i));
      MarkSitEntryDirty(segno);
    }
    UpdateSitsInCursum(*sum, -SitsInCursum(*sum));
    return true;
  }
  return false;
}

zx_status_t SegmentManager::SetSummaryBlock(CursegType type, SummaryCallback callback) {
  CursegInfo *curseg = CURSEG_I(type);
  std::lock_guard curseg_lock(curseg->curseg_mutex);
  SummaryBlock *sum = curseg->sum_blk.get<SummaryBlock>();
  return callback(*sum);
}

zx_status_t SegmentManager::GetSummaryBlock(CursegType type, SummaryCallback callback) {
  CursegInfo *curseg = CURSEG_I(type);
  fs::SharedLock curseg_lock(curseg->curseg_mutex);
  SummaryBlock *sum = curseg->sum_blk.get<SummaryBlock>();
  return callback(*sum);
}

// CP calls this function, which flushes SIT entries including SitJournal,
// and moves prefree segs to free segs.
zx_status_t SegmentManager::FlushSitEntries() {
  block_t nsegs = TotalSegs();
  LockedPage page;
  zx_status_t status = SetSummaryBlock(CursegType::kCursegColdData, [&](SummaryBlock &sum) {
    uint32_t start = 0, end = 0;
    std::lock_guard sentry_lock(sentry_lock_);
    RawBitmap &bitmap = sit_info_->dirty_sentries_bitmap;
    // "flushed" indicates whether sit entries in journal are flushed
    // to the SIT area or not.
    bool flushed = FlushSitsInJournal();

    for (size_t sentry = 0; !bitmap.Scan(sentry, nsegs, false, &sentry); ++sentry) {
      uint32_t segno = safemath::checked_cast<uint32_t>(sentry);
      SegmentEntry &segment_entry = sit_info_->sentries[segno];
      uint32_t sit_offset;
      int offset = -1;

      sit_offset = SitEntryOffset(segno);
      uint32_t delta = segment_entry.ckpt_invalid_blocks;
      if (!flushed) {
        offset = LookupJournalInCursum(sum, JournalType::kSitJournal, segno, 1);
      }

      if (offset >= 0) {
        SetSegnoInJournal(sum, offset, CpuToLe(segno));
        SegInfoToRawSit(segment_entry, SitInJournal(sum, offset));
      } else {
        if (!page || (start > segno) || (segno > end)) {
          if (page) {
            page.SetDirty();
            page.reset();
          }

          start = StartSegNo(segno);
          end = start + kSitEntryPerBlock - 1;

          // read sit block that will be updated
          zx::result next = GetNextSitPage(start);
          if (next.is_error()) {
            return next.error_value();
          }
          page = *std::move(next);
        }

        // udpate entry in SIT block
        SegInfoToRawSit(segment_entry, page->GetAddress<SitBlock>()->entries[sit_offset]);
      }
      bitmap.ClearOne(segno);
      --sit_info_->dirty_sentries;
      // Update |ckpt_invalid_blocks_| if the checkpointed invalid blocks of dirty segments change.
      delta = segment_entry.ckpt_invalid_blocks - delta;
      if (delta && !IsCurSeg(segno) && segment_entry.valid_blocks &&
          segment_entry.valid_blocks < kDefaultBlocksPerSegment) {
        UpdateInvalidBlocks(delta, IsNodeSeg(static_cast<CursegType>(segment_entry.type)));
      }
    }
    return ZX_OK;
  });

  if (status != ZX_OK) {
    return status;
  }

  // Write out last modified SIT block
  if (page != nullptr) {
    page.SetDirty();
  }

  SetPrefreeAsFreeSegments();
  return ZX_OK;
}

zx_status_t SegmentManager::BuildSitInfo() {
  const Superblock &raw_super = superblock_info_.GetSuperblock();
  const Checkpoint &ckpt = superblock_info_.GetCheckpoint();
  uint32_t sit_segs;

  // allocate memory for SIT information
  sit_info_ = std::make_unique<SitInfo>();

  SitInfo *sit_i = sit_info_.get();
  if (sit_i->sentries = std::make_unique<SegmentEntry[]>(TotalSegs()); !sit_i->sentries) {
    return ZX_ERR_NO_MEMORY;
  }

  sit_i->dirty_sentries_bitmap.Reset(TotalSegs());

  for (uint32_t start = 0; start < TotalSegs(); ++start) {
    sit_i->sentries[start].cur_valid_map.Reset(GetBitSize(kSitVBlockMapSize));
    sit_i->sentries[start].ckpt_valid_map.Reset(GetBitSize(kSitVBlockMapSize));
  }

  if (superblock_info_.GetSegsPerSec() > 1) {
    if (sit_i->sec_entries = std::make_unique<SectionEntry[]>(superblock_info_.GetTotalSections());
        !sit_i->sec_entries) {
      return ZX_ERR_NO_MEMORY;
    }
  }

  uint32_t bitmap_size = superblock_info_.GetSitBitmapSize();
  // get information related with SIT
  sit_segs = LeToCpu(raw_super.segment_count_sit) >> 1;

  // setup SIT bitmap from ckeckpoint pack
  sit_i->sit_bitmap.Reset(GetBitSize(bitmap_size));
  CloneBits(sit_i->sit_bitmap, superblock_info_.GetSitBitmap(), 0, GetBitSize(bitmap_size));

#if 0  // porting needed
  /* init SIT information */
  // sit_i->s_ops = &default_salloc_ops;
#endif
  auto cur_time = time(nullptr);

  sit_i->sit_base_addr = LeToCpu(raw_super.sit_blkaddr);
  sit_i->sit_blocks = sit_segs << superblock_info_.GetLogBlocksPerSeg();
  sit_i->written_valid_blocks = LeToCpu(safemath::checked_cast<block_t>(ckpt.valid_block_count));
  sit_i->bitmap_size = bitmap_size;
  sit_i->dirty_sentries = 0;
  sit_i->elapsed_time = LeToCpu(superblock_info_.GetCheckpoint().elapsed_time);
  sit_i->mounted_time = cur_time;
  return ZX_OK;
}

zx_status_t SegmentManager::BuildFreeSegmap() {
  std::lock_guard segmap_lock(segmap_lock_);
  // allocate memory for free segmap information
  free_info_ = std::make_unique<FreeSegmapInfo>();

  if (zx_status_t status = free_info_->free_segmap.Reset(TotalSegs()); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = free_info_->free_secmap.Reset(superblock_info_.GetTotalSections());
      status != ZX_OK) {
    return status;
  }

  // set all segments as dirty temporarily
  free_info_->free_segmap.Set(0, TotalSegs());
  free_info_->free_secmap.Set(0, superblock_info_.GetTotalSections());

  // init free segmap information
  start_segno_ = GetSegNoFromSeg0(main_blkaddr_);
  free_info_->free_segments = 0;
  free_info_->free_sections = 0;

  return ZX_OK;
}

zx_status_t SegmentManager::BuildCurseg() {
  for (auto &curseg : curseg_array_) {
    memset(curseg.sum_blk.get(), 0, kBlockSize);
    curseg.segno = kNullSegNo;
    curseg.next_blkoff = 0;
  }
  return RestoreCursegSummaries();
}

zx_status_t SegmentManager::BuildSitEntries() {
  CursegInfo *curseg = CURSEG_I(CursegType::kCursegColdData);
  SummaryBlock *sum = &curseg->sum_blk;

  std::lock_guard sentry_lock(sentry_lock_);
  for (uint32_t start = 0; start < TotalSegs(); ++start) {
    SegmentEntry &segment_entry = sit_info_->sentries[start];
    SitBlock *sit_blk;
    SitEntry sit;
    bool got_it = false;
    {
      std::lock_guard curseg_lock(curseg->curseg_mutex);
      for (int i = 0; i < SitsInCursum(*sum); ++i) {
        if (LeToCpu(SegnoInJournal(*sum, i)) == start) {
          sit = SitInJournal(*sum, i);
          got_it = true;
          break;
        }
      }
    }
    if (!got_it) {
      zx::result page = GetCurrentSitPage(start);
      if (page.is_error()) {
        return page.error_value();
      }
      sit_blk = page->GetAddress<SitBlock>();
      sit = sit_blk->entries[SitEntryOffset(start)];
    }
    CheckBlockCount(start, sit);
    SegInfoFromRawSit(segment_entry, sit);
    if (superblock_info_.GetSegsPerSec() > 1) {
      SectionEntry &e = sit_info_->sec_entries[GetSecNo(start)];
      e.valid_blocks += segment_entry.valid_blocks;
    }
  }
  return ZX_OK;
}

void SegmentManager::InitFreeSegmap() {
  for (uint32_t start = 0; start < TotalSegs(); ++start) {
    SegmentEntry &sentry = sit_info_->sentries[start];
    if (!sentry.valid_blocks)
      SetFree(start);
  }

  // set use the current segments
  for (int type = static_cast<int>(CursegType::kCursegHotData);
       type <= static_cast<int>(CursegType::kCursegColdNode); ++type) {
    CursegInfo *curseg_t = CURSEG_I(static_cast<CursegType>(type));
    SetTestAndInuse(curseg_t->segno);
  }
}

zx_status_t SegmentManager::BuildDirtySegmap() {
  std::lock_guard lock(sentry_lock_);
  std::lock_guard seglist_lock(seglist_lock_);
  dirty_info_ = std::make_unique<DirtySeglistInfo>();
  for (uint32_t i = 0; i < static_cast<int>(DirtyType::kNrDirtytype); ++i) {
    dirty_info_->dirty_segmap[i].Reset(TotalSegs());
    dirty_info_->nr_dirty[i] = 0;
  }

  // find dirty segment based on free segmap
  for (uint32_t segno = 0; (segno = FindNextInuse(TotalSegs(), segno)) < TotalSegs(); ++segno) {
    size_t valid_blocks = GetValidBlocks(segno, false);
    if (valid_blocks >= superblock_info_.GetBlocksPerSeg() || !valid_blocks) {
      continue;
    }
    LocateDirtySegment(segno, DirtyType::kDirty);
  }
  return dirty_info_->victim_secmap.Reset(superblock_info_.GetTotalSections());
}

// Update min, max modified time for cost-benefit GC algorithm
void SegmentManager::InitMinMaxMtime() {
  std::lock_guard sentry_lock(sentry_lock_);

  sit_info_->min_mtime = LLONG_MAX;

  for (uint32_t segno = 0; segno < TotalSegs(); segno += superblock_info_.GetSegsPerSec()) {
    uint64_t mtime = 0;

    for (uint32_t i = 0; i < superblock_info_.GetSegsPerSec(); ++i) {
      mtime += sit_info_->sentries[segno + i].mtime;
    }

    mtime /= static_cast<uint64_t>(superblock_info_.GetSegsPerSec());

    if (sit_info_->min_mtime > mtime) {
      sit_info_->min_mtime = mtime;
    }
  }
  sit_info_->max_mtime = GetMtime();
}

zx_status_t SegmentManager::BuildSegmentManager() {
  const Superblock &raw_super = superblock_info_.GetSuperblock();
  const Checkpoint &ckpt = superblock_info_.GetCheckpoint();

  seg0_blkaddr_ = LeToCpu(raw_super.segment0_blkaddr);
  main_blkaddr_ = LeToCpu(raw_super.main_blkaddr);
  segment_count_ = LeToCpu(raw_super.segment_count);
  reserved_segments_ = LeToCpu(ckpt.rsvd_segment_count);
  ovp_segments_ = LeToCpu(ckpt.overprov_segment_count);
  main_segments_ = LeToCpu(raw_super.segment_count_main);
  ssa_blkaddr_ = LeToCpu(raw_super.ssa_blkaddr);

  if (zx_status_t ret = BuildSitInfo(); ret != ZX_OK) {
    return ret;
  }

  if (zx_status_t ret = BuildFreeSegmap(); ret != ZX_OK) {
    return ret;
  }

  if (zx_status_t ret = BuildCurseg(); ret != ZX_OK) {
    return ret;
  }

  // reinit free segmap based on SIT
  if (zx_status_t ret = BuildSitEntries(); ret != ZX_OK) {
    return ret;
  }

  InitFreeSegmap();
  if (zx_status_t ret = BuildDirtySegmap(); ret != ZX_OK) {
    return ret;
  }
  InitMinMaxMtime();
  return ZX_OK;
}

void SegmentManager::DestroyDirtySegmap() {
  std::lock_guard seglist_lock(seglist_lock_);
  if (!dirty_info_)
    return;

  for (int i = 0; i < static_cast<int>(DirtyType::kNrDirtytype); ++i) {
    dirty_info_->nr_dirty[i] = 0;
  }

  dirty_info_.reset();
}

void SegmentManager::DestroySegmentManager() { DestroyDirtySegmap(); }

void SegmentManager::UpdateInvalidBlocks(int32_t delta, bool is_node) {
  ckpt_invalid_blocks_[is_node] += delta;
  ZX_ASSERT(ckpt_invalid_blocks_[is_node] >= 0);
}

}  // namespace f2fs
