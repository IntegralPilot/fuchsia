// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/inspect.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/segment.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vnode.h"
#include "src/storage/f2fs/vnode_cache.h"
#include "src/storage/f2fs/writeback.h"

namespace f2fs {

zx_status_t F2fs::GrabMetaPage(pgoff_t index, LockedPage *out) {
  if (zx_status_t ret = GetMetaVnode().GrabLockedPage(index, out); ret != ZX_OK) {
    ZX_ASSERT_MSG(false, "GrabMetaPage() fails [addr: 0x%lx, ret: %d]\n", index, ret);
    return ret;
  }
  // We wait writeback only inside GrabMetaPage()
  (*out).WaitOnWriteback();
  (*out)->SetUptodate();
  return ZX_OK;
}

zx_status_t F2fs::GetMetaPage(pgoff_t index, LockedPage *out) {
  LockedPage page;
  if (zx_status_t ret = GetMetaVnode().GrabLockedPage(index, &page); ret != ZX_OK) {
    ZX_ASSERT_MSG(false, "GetMetaPage() fails [addr: 0x%lx, ret: %d]\n", index, ret);
    return ret;
  }

  if (auto status =
          MakeReadOperation(page, safemath::checked_cast<block_t>(index), PageType::kMeta);
      status.is_error()) {
    FX_LOGS(WARNING) << "GetMetaPage() fails. index: " << index
                     << zx_status_get_string(status.error_value());
    return status.status_value();
  }
#if 0  // porting needed
  // mark_page_accessed(page);
#endif
  *out = std::move(page);
  return ZX_OK;
}

pgoff_t F2fs::FlushDirtyMetaPages(bool is_commit) {
  if (superblock_info_->GetPageCount(CountType::kDirtyMeta) == 0) {
    return 0;
  }
  WritebackOperation operation = {.bReclaim = GetMemoryStatus(MemoryStatus::kNeedReclaim)};
  if (is_commit) {
    operation.bSync = true;
    operation.page_cb = [](fbl::RefPtr<Page> page, bool is_last_page) {
      if (is_last_page) {
        page->SetCommit();
      }
      return ZX_OK;
    };
    // Ensure that all pending Pages in |writer_| are written out on storage before the commit.
    writer_->Sync();
  }
  return GetMetaVnode().Writeback(operation);
}

zx_status_t F2fs::CheckOrphanSpace() {
  /*
   * considering 512 blocks in a segment 5 blocks are needed for cp
   * and log segment summaries. Remaining blocks are used to keep
   * orphan entries with the limitation one reserved segment
   * for cp pack we can have max 1020*507 orphan entries
   */
  size_t max_orphans = (superblock_info_->GetBlocksPerSeg() - 5) * kOrphansPerBlock;
  if (GetVnodeSetSize(VnodeSet::kOrphan) >= max_orphans) {
    inspect_tree_->OnOutOfSpace();
    return ZX_ERR_NO_SPACE;
  }
  return ZX_OK;
}

zx::result<> F2fs::PurgeOrphanInode(nid_t ino) {
  zx::result vnode = GetVnode(ino);
  if (vnode.is_error()) {
    return vnode.take_error();
  }
  vnode->ClearLinkCount();
  vnode->Purge();
  return zx::ok();
}

zx_status_t F2fs::PurgeOrphanInodes() {
  if (!(superblock_info_->TestCpFlags(CpFlag::kCpOrphanPresentFlag))) {
    return ZX_OK;
  }
  block_t start_blk = superblock_info_->StartCpAddr() + superblock_info_->GetNumCpPayload() + 1;
  block_t start_sum = superblock_info_->StartSumAddr();
  block_t num_cp_payload = superblock_info_->GetNumCpPayload();
  if (start_sum <= num_cp_payload) {
    return ZX_ERR_BAD_STATE;
  }
  block_t orphan_blocks = start_sum - 1 - num_cp_payload;
  SetOnRecovery();
  auto cleanup = fit::defer([this] { ClearOnRecovery(); });

  fs::SharedLock lock(f2fs::GetGlobalLock());
  for (block_t i = 0; i < orphan_blocks; ++i) {
    LockedPage page;
    if (zx_status_t ret = GetMetaPage(start_blk + i, &page); ret != ZX_OK) {
      return ret;
    }

    OrphanBlock *orphan_blk;

    orphan_blk = page->GetAddress<OrphanBlock>();
    uint32_t entry_count = LeToCpu(orphan_blk->entry_count);
    // TODO: Need to set NeedChkp flag to repair the fs when fsck repair is available.
    // For now, we trigger assertion.
    ZX_ASSERT(entry_count <= kOrphansPerBlock);
    for (block_t j = 0; j < entry_count; ++j) {
      nid_t ino = LeToCpu(orphan_blk->ino[j]);
      if (PurgeOrphanInode(ino).is_error()) {
        FX_LOGS(WARNING) << "failed to purge an orphan file (ino: " << ino << ")";
      }
    }
  }
  // clear Orphan Flag
  superblock_info_->ClearCpFlags(CpFlag::kCpOrphanPresentFlag);
  return ZX_OK;
}

void F2fs::WriteOrphanInodes(block_t start_blk) {
  OrphanBlock *orphan_blk = nullptr;
  LockedPage page;
  uint32_t nentries = 0;
  uint16_t index = 1;
  uint16_t orphan_blocks = CheckedDivRoundUp(
      safemath::checked_cast<uint16_t>(GetVnodeSetSize(VnodeSet::kOrphan)), kOrphansPerBlock);

  ForAllVnodeSet(VnodeSet::kOrphan, [&](nid_t ino) {
    if (nentries == kOrphansPerBlock) {
      // an orphan block is full of 1020 entries,
      // then we need to flush current orphan blocks
      // and bring another one in memory
      orphan_blk->blk_addr = CpuToLe(index);
      orphan_blk->blk_count = CpuToLe(orphan_blocks);
      orphan_blk->entry_count = CpuToLe(nentries);
      page.SetDirty();
      page.reset();
      ++index;
      ++start_blk;
      nentries = 0;
    }
    if (!page) {
      GrabMetaPage(start_blk, &page);
      orphan_blk = page->GetAddress<OrphanBlock>();
      memset(orphan_blk, 0, sizeof(*orphan_blk));
      page.SetDirty();
    }
    orphan_blk->ino[nentries++] = CpuToLe(ino);
  });
  if (page) {
    orphan_blk->blk_addr = CpuToLe(index);
    orphan_blk->blk_count = CpuToLe(orphan_blocks);
    orphan_blk->entry_count = CpuToLe(nentries);
    page.SetDirty();
  }
}

zx_status_t F2fs::ValidateCheckpoint(block_t cp_addr, uint64_t *version, LockedPage *out) {
  uint64_t checkpoint_version = 0;
  constexpr size_t kFirstCheckpointHeaderBlock = 0;
  LockedPage header;
  for (size_t i = 0; i < kNumCheckpointHeaderBlocks; ++i) {
    LockedPage cp_page;
    // Read the header page in this checkpoint pack
    if (zx_status_t ret = GetMetaPage(cp_addr, &cp_page); ret != ZX_OK) {
      return ret;
    }
    // get the version number
    Checkpoint *cp_block = cp_page->GetAddress<Checkpoint>();
    zx::result crc = superblock_info_->GetCrcFromCheckpointBlock(cp_block);
    if (crc.is_error()) {
      return crc.error_value();
    }

    if (!F2fsCrcValid(*crc, cp_block, LeToCpu(cp_block->checksum_offset))) {
      return ZX_ERR_BAD_STATE;
    }
    // The checkpoint header and its copy are located in the first and last blocks on a checkpoint
    // pack respectively.
    if (i == kFirstCheckpointHeaderBlock) {
      uint32_t total_cp_blocks = LeToCpu(cp_block->cp_pack_total_block_count);
      if (total_cp_blocks <= 2 || total_cp_blocks > superblock_info_->GetBlocksPerSeg()) {
        return ZX_ERR_BAD_STATE;
      }
      checkpoint_version = LeToCpu(cp_block->checkpoint_ver);
      header = std::move(cp_page);
      // Set |cp_addr| to read a copy of the header page in this checkpoint pack
      cp_addr += total_cp_blocks - 1;
    } else if (checkpoint_version != LeToCpu(cp_block->checkpoint_ver)) {
      return ZX_ERR_BAD_STATE;
    }
  }
  *version = checkpoint_version;
  *out = std::move(header);
  return ZX_OK;
}

zx_status_t F2fs::GetValidCheckpoint() {
  LockedPage cp1, cp2;
  fbl::RefPtr<Page> cur_page;
  uint64_t cp1_version = 0, cp2_version = 0;
  block_t cp_start_blk_no;

  /*
   * Finding out valid cp block involves read both
   * sets( cp pack1 and cp pack 2)
   */
  block_t start_addr = LeToCpu(superblock_info_->GetSuperblock().cp_blkaddr);
  cp_start_blk_no = start_addr;
  ValidateCheckpoint(cp_start_blk_no, &cp1_version, &cp1);

  /* The second checkpoint pack should start at the next segment */
  cp_start_blk_no += 1 << superblock_info_->GetLogBlocksPerSeg();
  ValidateCheckpoint(cp_start_blk_no, &cp2_version, &cp2);

  BlockBuffer<Checkpoint> checkpoint_block;
  if (cp1 && cp2) {
    if (VerAfter(cp2_version, cp1_version)) {
      cp2->Read(checkpoint_block.get(), 0, kBlockSize);
      if (superblock_info_->SetCheckpoint(checkpoint_block) != ZX_OK) {
        cur_page = cp1.CopyRefPtr();
        cp_start_blk_no = start_addr;
      }
    } else {
      cp1->Read(checkpoint_block.get(), 0, kBlockSize);
      if (superblock_info_->SetCheckpoint(checkpoint_block) != ZX_OK) {
        cur_page = cp2.CopyRefPtr();
      } else {
        cp_start_blk_no = start_addr;
      }
    }
  } else if (cp1) {
    cur_page = cp1.CopyRefPtr();
    cp_start_blk_no = start_addr;
  } else if (cp2) {
    cur_page = cp2.CopyRefPtr();
  } else {
    return ZX_ERR_INVALID_ARGS;
  }

  if (cur_page) {
    cur_page->Read(checkpoint_block.get(), 0, kBlockSize);
    if (zx_status_t status = superblock_info_->SetCheckpoint(checkpoint_block); status != ZX_OK) {
      return status;
    }
  }

  block_t num_payload = superblock_info_->GetNumCpPayload();
  if (num_payload) {
    size_t blk_size = superblock_info_->GetBlocksize();
    superblock_info_->SetExtraSitBitmap(num_payload * blk_size);
    for (uint32_t i = 0; i < num_payload; ++i) {
      LockedPage cp_page;
      if (zx_status_t ret = GetMetaPage(cp_start_blk_no + 1 + i, &cp_page); ret != ZX_OK) {
        return ret;
      }
      CloneBits(superblock_info_->GetExtraSitBitmap(), cp_page->GetAddress(),
                GetBitSize(i * blk_size), GetBitSize(blk_size));
    }
  }

  return ZX_OK;
}

pgoff_t F2fs::FlushDirtyDataPages(WritebackOperation &operation) {
  pgoff_t total_nwritten = 0;
  GetVCache().ForDirtyVnodesIf(
      [&](fbl::RefPtr<VnodeF2fs> &vnode) TA_NO_THREAD_SAFETY_ANALYSIS {
        if (vnode->GetDirtyPageCount()) {
          auto nwritten = vnode->Writeback(operation);
          total_nwritten = safemath::CheckAdd<pgoff_t>(total_nwritten, nwritten).ValueOrDie();
          if (nwritten >= operation.to_write) {
            return ZX_ERR_STOP;
          }
          operation.to_write -= nwritten;
        }
        return ZX_OK;
      },
      std::move(operation.if_vnode));
  return total_nwritten;
}

zx::result<pgoff_t> F2fs::FlushDirtyNodePages(WritebackOperation &operation) {
  if (zx_status_t status = GetVCache().ForDirtyVnodesIf(
          [](fbl::RefPtr<VnodeF2fs> &vnode) {
            ZX_ASSERT(vnode->ClearDirty());
            return ZX_OK;
          },
          [this](fbl::RefPtr<VnodeF2fs> &vnode) {
            if (vnode->GetDirtyPageCount() && vnode->GetLinkCount()) {
              return ZX_ERR_NEXT;
            }
            LockedPage node_page;
            if (zx_status_t ret = node_manager_->GetNodePage(vnode->GetKey(), &node_page);
                ret != ZX_OK) {
              return ret;
            }
            vnode->UpdateInodePage(node_page, true);
            if (!vnode->GetLinkCount()) {
              return ZX_ERR_NEXT;
            }
            return ZX_OK;
          });
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(GetNodeVnode().Writeback(operation));
}

void F2fs::FlushDirsAndNodes() {
  // Here, we just schedule dirty Pages to be written back on storage.
  do {
    // Write out all dirty dentry pages and remove orphans from dirty list.
    WritebackOperation op = {.bReclaim = GetMemoryStatus(MemoryStatus::kNeedReclaim)};
    op.if_vnode = [](fbl::RefPtr<VnodeF2fs> &vnode) {
      if (vnode->IsDir()) {
        return ZX_OK;
      }
      return ZX_ERR_NEXT;
    };
    FlushDirtyDataPages(op);
  } while (superblock_info_->GetPageCount(CountType::kDirtyDents));

  // POR: we should ensure that there is no dirty node pages
  // until finishing nat/sit flush.
  do {
    WritebackOperation op = {.bReclaim = GetMemoryStatus(MemoryStatus::kNeedReclaim)};
    auto ret = FlushDirtyNodePages(op);
    ZX_ASSERT_MSG(ret.is_ok(), "Failed to flush node pages (%ul) %s",
                  superblock_info_->GetPageCount(CountType::kDirtyNodes), ret.status_string());
  } while (superblock_info_->GetPageCount(CountType::kDirtyNodes));
}

zx_status_t F2fs::DoCheckpoint(bool is_umount) {
  // Flush all the NAT/SIT pages
  SuperblockInfo &superblock_info = GetSuperblockInfo();
  while (superblock_info.GetPageCount(CountType::kDirtyMeta)) {
    FlushDirtyMetaPages(false);
  }
  if (is_umount) {
    FX_LOGS(INFO) << "flushing dirty metadata… done";
  }

  GetWriter().ScheduleWriteBlocks();

  auto &ckpt_block = superblock_info.GetCheckpointBlock();
  if (zx::result last_nid = GetNodeManager().GetNextFreeNid(); last_nid.is_ok()) {
    ckpt_block->next_free_nid = CpuToLe(*last_nid);
  } else {
    ckpt_block->next_free_nid = CpuToLe(GetNodeManager().GetNextScanNid());
  }

  // modify checkpoint
  // version number is already updated
  ckpt_block->elapsed_time = CpuToLe(GetSegmentManager().GetMtime());
  ckpt_block->valid_block_count = CpuToLe(superblock_info.GetValidBlockCount());
  ckpt_block->free_segment_count = CpuToLe(GetSegmentManager().FreeSegments());
  constexpr CursegType kNodeLogs[] = {CursegType::kCursegHotNode, CursegType::kCursegWarmNode,
                                      CursegType::kCursegColdNode};
  for (size_t i = 0; i < std::size(kNodeLogs); ++i) {
    CursegType type = kNodeLogs[i];
    ckpt_block->cur_node_segno[i] = CpuToLe(GetSegmentManager().CursegSegno(type));
    ckpt_block->cur_node_blkoff[i] = CpuToLe(GetSegmentManager().CursegBlkoff(type));
    ckpt_block->alloc_type[static_cast<size_t>(type)] =
        static_cast<uint8_t>(GetSegmentManager().CursegAllocType(type));
  }
  constexpr CursegType kDataLogs[] = {CursegType::kCursegHotData, CursegType::kCursegWarmData,
                                      CursegType::kCursegColdData};
  for (size_t i = 0; i < std::size(kDataLogs); ++i) {
    CursegType type = kDataLogs[i];
    ckpt_block->cur_data_segno[i] = CpuToLe(GetSegmentManager().CursegSegno(type));
    ckpt_block->cur_data_blkoff[i] = CpuToLe(GetSegmentManager().CursegBlkoff(type));
    ckpt_block->alloc_type[static_cast<size_t>(type)] =
        static_cast<uint8_t>(GetSegmentManager().CursegAllocType(type));
  }

  ckpt_block->valid_node_count = CpuToLe(superblock_info.GetValidNodeCount());
  ckpt_block->valid_inode_count = CpuToLe(superblock_info.GetValidInodeCount());

  // 2 cp  + n data seg summary + orphan inode blocks
  uint32_t data_sum_blocks = GetSegmentManager().NpagesForSummaryFlush();
  if (data_sum_blocks < 3) {
    superblock_info.SetCpFlags(CpFlag::kCpCompactSumFlag);
  } else {
    superblock_info.ClearCpFlags(CpFlag::kCpCompactSumFlag);
  }

  block_t num_cp_payload = superblock_info.GetNumCpPayload();
  uint16_t orphan_blocks = CheckedDivRoundUp(
      safemath::checked_cast<uint16_t>(GetVnodeSetSize(VnodeSet::kOrphan)), kOrphansPerBlock);
  uint32_t cp_pack_total_block_count = 2 + data_sum_blocks + orphan_blocks + num_cp_payload;
  if (is_umount) {
    superblock_info.SetCpFlags(CpFlag::kCpUmountFlag);
    cp_pack_total_block_count += kNrCursegNodeType;
  } else {
    superblock_info.ClearCpFlags(CpFlag::kCpUmountFlag);
  }
  ckpt_block->cp_pack_start_sum = CpuToLe(1 + orphan_blocks + num_cp_payload);
  ckpt_block->cp_pack_total_block_count = CpuToLe(cp_pack_total_block_count);

  if (GetVnodeSetSize(VnodeSet::kOrphan) > 0) {
    superblock_info.SetCpFlags(CpFlag::kCpOrphanPresentFlag);
  } else {
    superblock_info.ClearCpFlags(CpFlag::kCpOrphanPresentFlag);
  }

  // Make use crc and version for recovery
  superblock_info.SetCpFlags(CpFlag::kCpCrcRecoveryFlag);

  // update SIT/NAT bitmap
  GetSegmentManager().GetSitBitmap(superblock_info.GetSitBitmap());
  GetNodeManager().GetNatBitmap(superblock_info.GetNatBitmap());

  uint32_t crc32 = CpuToLe(F2fsCrc32(&ckpt_block, LeToCpu(ckpt_block->checksum_offset)));
  std::memcpy(ckpt_block.get<uint8_t>() + LeToCpu(ckpt_block->checksum_offset), &crc32,
              sizeof(uint32_t));

  block_t start_blk = superblock_info.StartCpAddr();

  // Prepare Pages for this checkpoint pack
  {
    LockedPage cp_page;
    GrabMetaPage(start_blk++, &cp_page);
    cp_page->Write(&ckpt_block, 0, superblock_info.GetBlocksize());
    cp_page.SetDirty();
  }

  size_t offset = 0;
  for (size_t i = 0; i < num_cp_payload; ++i) {
    LockedPage cp_page;
    GrabMetaPage(start_blk++, &cp_page);
    memcpy(cp_page->GetAddress(), &superblock_info.GetSitBitmap()[offset], cp_page->Size());
    cp_page.SetDirty();
    offset += cp_page->Size();
  }

  if (GetVnodeSetSize(VnodeSet::kOrphan) > 0) {
    WriteOrphanInodes(start_blk);
    start_blk += orphan_blocks;
  }

  GetSegmentManager().WriteDataSummaries(start_blk);
  start_blk += data_sum_blocks;
  if (is_umount) {
    GetSegmentManager().WriteNodeSummaries(start_blk);
    start_blk += kNrCursegNodeType;
  }

  // Write out this checkpoint pack.
  FlushDirtyMetaPages(false);

  // Prepare the commit block.
  {
    LockedPage cp_page;
    GrabMetaPage(start_blk, &cp_page);
    cp_page->Write(&ckpt_block, 0, superblock_info.GetBlocksize());
    cp_page.SetDirty();
  }

  // Update the valid block count.
  superblock_info.ResetAllocBlockCount();

  // Commit.
  if (!superblock_info.TestCpFlags(CpFlag::kCpErrorFlag)) {
    ZX_ASSERT(superblock_info.GetPageCount(CountType::kDirtyMeta) == 1);
    FlushDirtyMetaPages(true);
    if (superblock_info_->TestCpFlags(CpFlag::kCpErrorFlag)) {
      return ZX_ERR_UNAVAILABLE;
    }
    GetSegmentManager().ClearPrefreeSegments();
    superblock_info.ClearDirty();
    meta_vnode_->CleanupCache();
    ClearVnodeSet();
  }
  return ZX_OK;
}

// Release-acquire ordering between the writeback (loader) and others such as checkpoint and gc.
bool F2fs::IsTearDown() const { return teardown_flag_.test(std::memory_order_relaxed); }
void F2fs::SetTearDown() { teardown_flag_.test_and_set(std::memory_order_relaxed); }

// We guarantee that this checkpoint procedure should not fail.
zx_status_t F2fs::WriteCheckpoint(bool is_umount) {
  std::lock_guard gc_lock(f2fs::GetGlobalLock());
  return WriteCheckpointUnsafe(is_umount);
}

zx_status_t F2fs::WriteCheckpointUnsafe(bool is_umount) {
  if (superblock_info_->TestCpFlags(CpFlag::kCpErrorFlag)) {
    return ZX_ERR_BAD_STATE;
  }
  FlushDirsAndNodes();
  if (is_umount) {
    FX_LOGS(INFO) << "flushing dirty nodes… done";
  }

  // update checkpoint pack index
  // Increase the version number so that
  // SIT entries and seg summaries are written at correct place
  superblock_info_->UpdateCheckpointVer();

  // write cached NAT/SIT entries to NAT/SIT area
  if (zx_status_t ret = GetNodeManager().FlushNatEntries(); ret != ZX_OK) {
    return ret;
  }
  if (zx_status_t ret = GetSegmentManager().FlushSitEntries(); ret != ZX_OK) {
    return ret;
  }

  // unlock all the fs_lock[] in do_checkpoint()
  if (zx_status_t ret = DoCheckpoint(is_umount); ret != ZX_OK) {
    return ret;
  }

  // Here, we ensure that all writeback pages are written out.
  if (GetMemoryStatus(MemoryStatus::kNeedReclaim) || is_umount) {
    // Clear clean pages for all vnodes, and remove inactive vnodes that have no pages.
    node_vnode_->CleanupCache();
    GetVCache().Shrink();
  }

  if (is_umount && !(superblock_info_->TestCpFlags(CpFlag::kCpErrorFlag))) {
    ZX_ASSERT(superblock_info_->GetPageCount(CountType::kDirtyDents) == 0);
    ZX_ASSERT(superblock_info_->GetPageCount(CountType::kDirtyData) == 0);
    ZX_ASSERT(superblock_info_->GetPageCount(CountType::kWriteback) == 0);
    ZX_ASSERT(superblock_info_->GetPageCount(CountType::kDirtyMeta) == 0);
    ZX_ASSERT(superblock_info_->GetPageCount(CountType::kDirtyNodes) == 0);
  }
  return ZX_OK;
}

}  // namespace f2fs
