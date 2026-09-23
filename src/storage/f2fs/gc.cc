// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/segment.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vnode.h"
#include "src/storage/f2fs/writeback.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {
size_t SegmentManager::GetGcCost(uint32_t segno, const VictimSelPolicy &policy) const {
  if (policy.alloc_type == AllocType::kSSR) {
    return sit_info_->sentries[segno].ckpt_valid_blocks;
  }

  if (policy.gc_mode == GcMode::kGcGreedy) {
    return GetValidBlocks(segno, true);
  }
  return GetCostBenefitRatio(segno);
}

size_t SegmentManager::GetCostBenefitRatio(uint32_t segno) const {
  size_t start = GetSecNo(segno) * superblock_info_.GetSegsPerSec();
  uint64_t mtime = 0;
  for (size_t i = 0; i < superblock_info_.GetSegsPerSec(); i++) {
    mtime += sit_info_->sentries[start + i].mtime;
  }
  mtime = mtime / superblock_info_.GetSegsPerSec();
  size_t valid_blocks_ratio = 100 * GetValidBlocks(segno, true) / superblock_info_.GetSegsPerSec() /
                              superblock_info_.GetBlocksPerSeg();

  // Handle if the system time is changed by user
  if (mtime < sit_info_->min_mtime) {
    sit_info_->min_mtime = mtime ? mtime - 1 : 0;
  }
  if (mtime > sit_info_->max_mtime) {
    sit_info_->max_mtime = std::max(GetMtime(), mtime + 1);
  }

  // A higher value means that it's been longer since it was last modified.
  size_t age = 0;
  if (sit_info_->max_mtime != sit_info_->min_mtime) {
    age = 100 -
          (100 * (mtime - sit_info_->min_mtime) / (sit_info_->max_mtime - sit_info_->min_mtime));
  }

  return kUint32Max - (100 * (100 - valid_blocks_ratio) * age / (100 + valid_blocks_ratio));
}

VictimSelPolicy SegmentManager::GetVictimSelPolicy(GcType gc_type, CursegType type,
                                                   AllocType alloc_type) const {
  VictimSelPolicy policy;
  policy.alloc_type = alloc_type;
  if (policy.alloc_type == AllocType::kSSR) {
    policy.gc_mode = GcMode::kGcGreedy;
    policy.dirty_segmap = &dirty_info_->dirty_segmap[static_cast<int>(type)];
    policy.max_search = dirty_info_->nr_dirty[static_cast<int>(type)];
    policy.ofs_unit = 1;
  } else {
    policy.gc_mode = (gc_type == GcType::kBgGc) ? GcMode::kGcCb : GcMode::kGcGreedy;
    policy.dirty_segmap = &dirty_info_->dirty_segmap[static_cast<int>(DirtyType::kDirty)];
    policy.max_search = dirty_info_->nr_dirty[static_cast<int>(DirtyType::kDirty)];
    policy.ofs_unit = superblock_info_.GetSegsPerSec();
  }

  if (policy.max_search > kMaxSearchLimit) {
    policy.max_search = kMaxSearchLimit;
  }

  policy.offset = last_victim_[static_cast<int>(policy.gc_mode)];
  return policy;
}

size_t SegmentManager::GetMaxCost(const VictimSelPolicy &policy) const {
  if (policy.alloc_type == AllocType::kSSR)
    return 1 << superblock_info_.GetLogBlocksPerSeg();
  if (policy.gc_mode == GcMode::kGcGreedy)
    return 2 * (1 << superblock_info_.GetLogBlocksPerSeg()) * policy.ofs_unit;
  else if (policy.gc_mode == GcMode::kGcCb)
    return kUint32Max;
  return 0;
}

uint32_t SegmentManager::GetBackgroundVictim() const {
  const size_t last = superblock_info_.GetTotalSections();
  // If the gc_type is GcType::kFgGc, we can select victim segments
  // selected by background GC before.
  // Those segments might have smaller valid blocks to be migrated.
  size_t secno = 0;
  while (!dirty_info_->victim_secmap.Scan(secno, last, false, &secno)) {
    if (SecUsageCheck(static_cast<uint32_t>(secno))) {
      ++secno;
      continue;
    }
    return safemath::checked_cast<uint32_t>(secno);
  }
  return kNullSegNo;
}

zx::result<uint32_t> SegmentManager::GetVictimByDefault(GcType gc_type, CursegType type,
                                                        AllocType alloc_type) {
  std::lock_guard lock(seglist_lock_);
  VictimSelPolicy policy = GetVictimSelPolicy(gc_type, type, alloc_type);

  policy.min_segno = kNullSegNo;
  policy.min_cost = GetMaxCost(policy);

  uint32_t nSearched = 0;

  if (policy.max_search == 0) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }

  if (policy.alloc_type == AllocType::kLFS && gc_type == GcType::kFgGc) {
    uint32_t secno = GetBackgroundVictim();
    if (secno != kNullSegNo) {
      dirty_info_->victim_secmap.ClearOne(secno);
      policy.min_segno = safemath::checked_cast<uint32_t>(secno * superblock_info_.GetSegsPerSec());
    }
  }

  auto &last_victim = last_victim_[static_cast<int>(policy.gc_mode)];
  auto gc_mode = static_cast<int>(policy.gc_mode);
  if (policy.min_segno == kNullSegNo) {
    block_t last_segment = TotalSegs();
    while (nSearched < policy.max_search) {
      size_t dirty_seg;
      if (policy.dirty_segmap->Scan(policy.offset, last_segment, false, &dirty_seg)) {
        if (!last_victim) {
          break;
        }
        last_segment = last_victim;
        last_victim = 0;
        policy.offset = 0;
        continue;
      }
      uint32_t segno = safemath::checked_cast<uint32_t>(dirty_seg);
      policy.offset = segno + policy.ofs_unit;
      uint32_t secno = GetSecNo(segno);

      if (policy.ofs_unit > 1) {
        policy.offset = fbl::round_down(policy.offset, policy.ofs_unit);
        nSearched +=
            CountBits(*policy.dirty_segmap, policy.offset - policy.ofs_unit, policy.ofs_unit);
      } else {
        ++nSearched;
      }

      if (SecUsageCheck(secno)) {
        continue;
      }
      if (gc_type == GcType::kBgGc && dirty_info_->victim_secmap.GetOne(secno)) {
        continue;
      }

      size_t cost = GetGcCost(segno, policy);

      if (policy.min_cost > cost) {
        policy.min_segno = segno;
        policy.min_cost = cost;
      }

      if (cost == GetMaxCost(policy)) {
        continue;
      }

      if (nSearched >= policy.max_search) {
        // It has already checked all or |kMaxSearchLimit| of dirty segments. Set the last victim to
        // |policy.offset| from which the next search will start to find victims.
        last_victim_[static_cast<int>(gc_mode)] =
            (safemath::CheckAdd<uint32_t>(segno, 1) % TotalSegs()).ValueOrDie();
      }
    }
  }

  if (policy.min_segno != kNullSegNo) {
    if (policy.alloc_type == AllocType::kLFS) {
      uint32_t secno = GetSecNo(policy.min_segno);
      if (gc_type == GcType::kFgGc) {
        cur_victim_sec_ = secno;
      } else {
        dirty_info_->victim_secmap.SetOne(secno);
      }
    }
    return zx::ok(
        safemath::checked_cast<uint32_t>(policy.min_segno - (policy.min_segno % policy.ofs_unit)));
  }

  return zx::error(ZX_ERR_UNAVAILABLE);
}

zx::result<uint32_t> SegmentManager::GetGcVictim(GcType gc_type, CursegType type) {
  fs::SharedLock sentry_lock(sentry_lock_);
  return GetVictimByDefault(gc_type, type, AllocType::kLFS);
}

bool SegmentManager::IsValidBlock(uint32_t segno, uint64_t offset) {
  fs::SharedLock sentry_lock(sentry_lock_);
  ZX_ASSERT(IsValidSegmentNumber(segno));
  return sit_info_->sentries[segno].cur_valid_map.GetOne(ToMsbFirst(offset));
}

zx_status_t SegmentManager::DoGarbageCollect(uint32_t start_segno, GcType gc_type) {
  for (uint32_t i = 0; i < superblock_info_.GetSegsPerSec(); ++i) {
    uint32_t segno = start_segno + i;
    uint8_t type = IsDataSeg(static_cast<CursegType>(GetSegmentEntry(segno).type)) ? kSumTypeData
                                                                                   : kSumTypeNode;

    if (CompareValidBlocks(0, segno, false)) {
      continue;
    }

    LockedPage sum_page;
    if (zx_status_t ret = GetSumPage(segno, &sum_page); ret != ZX_OK) {
      return ret;
    }

    SummaryBlock *sum_blk = sum_page->GetAddress<SummaryBlock>();
    ZX_DEBUG_ASSERT(type == GetSumType((&sum_blk->footer)));

    if (zx_status_t status = (type == kSumTypeNode) ? GcNodeSegment(*sum_blk, segno, gc_type)
                                                    : GcDataSegment(*sum_blk, segno, gc_type);
        status != ZX_OK) {
      return status;
    }
  }
  return ZX_OK;
}

zx_status_t SegmentManager::GcNodeSegment(const SummaryBlock &sum_blk, uint32_t segno,
                                          GcType gc_type) {
  const Summary *entry = sum_blk.entries;
  PageList pages_to_disk;
  for (block_t off = 0; off < superblock_info_.GetBlocksPerSeg(); ++off, ++entry) {
    nid_t nid = CpuToLe(entry->nid);

    if (gc_type == GcType::kBgGc && HasNotEnoughFreeSecs()) {
      return ZX_ERR_BAD_STATE;
    }

    if (!IsValidBlock(segno, off)) {
      continue;
    }

    LockedPage node_page;
    if (auto err = fs_->GetNodeManager().GetNodePage(nid, &node_page); err != ZX_OK) {
      continue;
    }

    NodeInfo ni;
    fs_->GetNodeManager().GetNodeInfo(nid, ni);
    if (ni.blk_addr != StartBlock(segno) + off) {
      continue;
    }

    node_page.WaitOnWriteback();
    node_page.SetDirty();
    if (gc_type == GcType::kFgGc) {
      block_t addr = fs_->GetNodeVnode().GetBlockAddr(node_page);
      if (addr == kNullAddr) {
        continue;
      }
      ZX_DEBUG_ASSERT(addr != kNewAddr);
      node_page.SetWriteback(addr);
      pages_to_disk.push_back(node_page.release());
    }
  }
  if (!pages_to_disk.is_empty()) {
    fs_->GetWriter().ScheduleWriteBlocks(nullptr, std::move(pages_to_disk));
  }

  return ZX_OK;
}

zx::result<std::pair<nid_t, block_t>> SegmentManager::CheckDnode(const Summary &sum,
                                                                 block_t blkaddr) {
  nid_t nid = LeToCpu(sum.nid);
  uint16_t ofs_in_node = LeToCpu(sum.ofs_in_node);

  LockedPage locked_page;
  if (auto err = fs_->GetNodeManager().GetNodePage(nid, &locked_page); err != ZX_OK) {
    return zx::error(err);
  }

  NodeInfo dnode_info;
  fs_->GetNodeManager().GetNodeInfo(nid, dnode_info);

  if (sum.version != dnode_info.version) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  fs_->GetNodeManager().CheckNidRange(dnode_info.ino);

  fbl::RefPtr<NodePage> node_page = fbl::RefPtr<NodePage>::Downcast(locked_page.CopyRefPtr());
  zx::result vnode = fs_->GetVnode(dnode_info.ino, node_page->IsInode() ? &locked_page : nullptr);
  if (vnode.is_error()) {
    return vnode.take_error();
  }

  auto start_bidx = node_page->StartBidxOfNode(vnode->GetAddrsPerInode());
  block_t source_blkaddr = node_page->GetBlockAddr(ofs_in_node);
  if (source_blkaddr != blkaddr) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  return zx::ok(std::make_pair(dnode_info.ino, start_bidx));
}

zx_status_t SegmentManager::GcDataSegment(const SummaryBlock &sum_blk, unsigned int segno,
                                          GcType gc_type) {
  block_t start_addr = StartBlock(segno);
  const Summary *entry = sum_blk.entries;
  PageList pages_to_disk;
  for (block_t off = 0; off < superblock_info_.GetBlocksPerSeg(); ++off, ++entry) {
    // stop BG_GC if there is not enough free sections. Or, stop GC if the section becomes fully
    // valid caused by race condition along with SSR block allocation.
    const uint32_t kBlocksPerSection =
        superblock_info_.GetBlocksPerSeg() * superblock_info_.GetSegsPerSec();
    const block_t target_address = safemath::CheckAdd<block_t>(start_addr, off).ValueOrDie();
    if ((gc_type == GcType::kBgGc && HasNotEnoughFreeSecs()) ||
        CompareValidBlocks(kBlocksPerSection, segno, true)) {
      return ZX_ERR_BAD_STATE;
    }

    if (!IsValidBlock(segno, off)) {
      continue;
    }

    auto dnode_result = CheckDnode(*entry, target_address);
    if (dnode_result.is_error()) {
      continue;
    }
    auto [ino, start_bidx] = dnode_result.value();

    uint32_t ofs_in_node = LeToCpu(entry->ofs_in_node);

    zx::result vnode = fs_->GetVnode(ino);
    if (vnode.is_error()) {
      continue;
    }
    // Migrate blocks only when their vnodes are valid.
    zx::result page = vnode->FindVictimPage(start_bidx + ofs_in_node);
    if (page.is_error()) {
      continue;
    }
    page->SetDirty();
    page->SetColdData();
    if (gc_type == GcType::kFgGc) {
      block_t addr = vnode->GetBlockAddr(*page);
      if (addr == kNullAddr) {
        continue;
      }
      ZX_DEBUG_ASSERT(addr != kNewAddr);
      (*page).SetWriteback(addr);
      pages_to_disk.push_back((*page).release());
    }
  }
  if (!pages_to_disk.is_empty()) {
    fs_->GetWriter().ScheduleWriteBlocks(nullptr, std::move(pages_to_disk));
  }

  if (gc_type == GcType::kFgGc && !CompareValidBlocks(0, segno, false)) {
    return ZX_ERR_BAD_STATE;
  }
  return ZX_OK;
}

}  // namespace f2fs
