// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <numeric>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/inspect.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/segment.h"
#include "src/storage/f2fs/vnode.h"

namespace f2fs {

static constexpr size_t kInvalidNodeOffset = std::numeric_limits<size_t>::max();
// Both offsets must come from NodePath, which derives them from the requested file offset.
// Caching an on-disk OfsOfNode() instead would let a forged footer collide with a deeper
// level's offset and reuse the wrong node without traversing to it.
static bool IsSameDnode(NodePath &path, size_t node_offset) {
  if (node_offset == kInvalidNodeOffset) {
    return false;
  }
  return path.node_offset[path.depth] == node_offset;
}

zx_status_t VnodeF2fs::ReserveNewBlock(LockedPage &node_page, size_t ofs_in_node) {
  if (TestFlag(InodeInfoFlag::kNoAlloc)) {
    return ZX_ERR_ACCESS_DENIED;
  }
  if (zx_status_t ret = superblock_info_.IncValidBlockCount(1); ret != ZX_OK) {
    if (ret == ZX_ERR_NO_SPACE) {
      fs()->GetInspectTree().OnOutOfSpace();
    }
    return ret;
  }

  node_page.WaitOnWriteback();
  node_page.GetPage<NodePage>().SetDataBlkaddr(ofs_in_node, kNewAddr);
  node_page.SetDirty();
  SetDirty();
  return ZX_OK;
}

void VnodeF2fs::UpdateExtentCache(pgoff_t file_offset, block_t blk_addr, uint32_t len) {
  if (!ExtentCacheAvailable()) {
    return;
  }

  if (zx::result result = extent_tree_->InsertExtent(ExtentInfo{file_offset, blk_addr, len});
      result.is_error()) {
    SetFlag(InodeInfoFlag::kNoExtent);
    return;
  }

  SetDirty();
}

zx::result<block_t> VnodeF2fs::LookupExtentCacheBlock(pgoff_t file_offset) {
  if (!ExtentCacheAvailable()) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }

  zx::result extent_info = extent_tree_->LookupExtent(file_offset);
  if (extent_info.is_error()) {
    return extent_info.take_error();
  }

  return zx::ok(extent_info->blk_addr +
                safemath::checked_cast<uint32_t>(file_offset - extent_info->fofs));
}

zx_status_t VnodeF2fs::GetNewDataPage(pgoff_t index, LockedPage *out) {
  block_t data_blkaddr;
  {
    zx::result path = GetNodePath(index);
    if (path.is_error()) {
      return path.status_value();
    }
    zx::result dnode_page = fs()->GetNodeManager().GetLockedDnodePage(*path, IsDir());
    if (dnode_page.is_error()) {
      return dnode_page.error_value();
    }
    AddBlocksUnsafe(path->num_new_nodes);
    size_t ofs_in_dnode = GetOfsInDnode(*path);
    data_blkaddr = (*dnode_page).GetPage<NodePage>().GetBlockAddr(ofs_in_dnode);
    if (data_blkaddr == kNullAddr) {
      if (zx_status_t ret = ReserveNewBlock(*dnode_page, ofs_in_dnode); ret != ZX_OK) {
        return ret;
      }
      AddBlocksUnsafe(1);
      data_blkaddr = kNewAddr;
    }
  }

  LockedPage page;
  if (zx_status_t ret = GrabLockedPage(index, &page); ret != ZX_OK) {
    return ret;
  }

  if (page->IsUptodate()) {
    *out = std::move(page);
    return ZX_OK;
  }

  if (data_blkaddr == kNewAddr) {
    page->SetUptodate();
    page.Zero();
  } else {
    ZX_ASSERT_MSG(data_blkaddr == kNewAddr, " %lu page should have kNewAddr but (0x%x)",
                  page->GetKey(), data_blkaddr);
  }

  *out = std::move(page);
  return ZX_OK;
}

#if 0  // porting needed
/**
 * This function should be used by the data read flow only where it
 * does not check the "create" flag that indicates block allocation.
 * The reason for this special functionality is to exploit VFS readahead
 * mechanism.
 */
// int VnodeF2fs::GetDataBlockRo(inode *inode, sector_t iblock,
//       buffer_head *bh_result, int create)
// {
//   uint32_t blkbits = inode->i_sb->s_blocksize_bits;
//   unsigned maxblocks = bh_result.value().b_size > blkbits;
//   DnodeOfData dn;
//   pgoff_t pgofs;
//   //int err = 0;

//   /* Get the page offset from the block offset(iblock) */
//   pgofs =  (pgoff_t)(iblock >> (kPageCacheShift - blkbits));

//   if (VnodeF2fs::CheckExtentCache(inode, pgofs, bh_result))
//     return 0;

//   /* When reading holes, we need its node page */
//   //TODO(unknown): inode should be replaced with vnodef2fs
//   //SetNewDnode(&dn, inode, nullptr, nullptr, 0);
//   // TODO(unknown): should be replaced with NodeManager->GetDnodeOfData
//   /*err = get_DnodeOfData(&dn, pgofs, kRdOnlyNode);
//   if (err)
//     return (err == ZX_ERR_NOT_FOUND) ? 0 : err; */

//   /* It does not support data allocation */
//   ZX_ASSERT(!create);

//   if (dn.data_blkaddr != kNewAddr && dn.data_blkaddr != kNullAddr) {
//     uint32_t end_offset;

//     end_offset = IsInode(dn.node_page) ?
//         kAddrsPerInode :
//         kAddrsPerBlock;

//     clear_buffer_new(bh_result);

//     /* Give more consecutive addresses for the read ahead */
//     for (uint32_t i = 0; i < end_offset - dn.ofs_in_node; ++i)
//       if (((DatablockAddr(dn.node_page,
//               dn.ofs_in_node + i))
//         != (dn.data_blkaddr + i)) || maxblocks == i)
//         break;
//     //map_bh(bh_result, inode->i_sb, dn.data_blkaddr);
//     bh_result->b_size = (i << blkbits);
//   }
//   F2fsPutDnode(&dn);
//   return 0;
// }
#endif

block_t VnodeF2fs::GetBlockAddr(LockedPage &page) {
  if (!page.ClearDirtyForIo()) {
    return kNullAddr;
  }
  if (IsMeta()) {
    block_t addr = safemath::checked_cast<block_t>(page->GetIndex());
    return addr;
  }
  ZX_DEBUG_ASSERT(IsNode());
  nid_t nid = page.GetPage<NodePage>().NidOfNode();
  ZX_DEBUG_ASSERT(page->GetIndex() == nid);

  NodeInfo ni;
  fs_->GetNodeManager().GetNodeInfo(nid, ni);
  block_t old_addr = ni.blk_addr;

  // This page is already truncated
  if (old_addr == kNullAddr) {
    return kNullAddr;
  }

  Summary sum;
  SetSummary(&sum, nid, 0, ni.version);
  block_t new_addr =
      fs_->GetSegmentManager().GetBlockAddrOnSegment(page, old_addr, &sum, PageType::kNode);
  ZX_DEBUG_ASSERT(new_addr != kNullAddr && new_addr != kNewAddr && new_addr != old_addr);

  fs_->GetNodeManager().SetNodeAddr(ni, new_addr);
  return new_addr;
}

block_t VnodeF2fs::GetBlockAddrOnDataSegment(LockedPage &page) {
  ZX_DEBUG_ASSERT(!IsMeta());
  ZX_DEBUG_ASSERT(!IsNode());
  if (!page.ClearDirtyForIo()) {
    return kNullAddr;
  }
  const pgoff_t end_index = GetSize() / kBlockSize;
  if (page->GetIndex() >= end_index) {
    unsigned offset = GetSize() & (kBlockSize - 1);
    if ((page->GetIndex() >= end_index + 1) || !offset) {
      return kNullAddr;
    }
  }
  zx::result path = GetNodePath(page->GetIndex());
  if (path.is_error()) {
    return kNullAddr;
  }

  zx::result dnode_page = fs()->GetNodeManager().FindLockedDnodePage(*path);
  if (dnode_page.is_error()) {
    if (page->IsUptodate() && dnode_page.status_value() != ZX_ERR_NOT_FOUND) {
      // In case of failure, we just redirty it.
      page.SetDirty();
      FX_LOGS(WARNING) << "failed to allocate a block." << dnode_page.status_string();
    }
    return kNullAddr;
  }

  size_t ofs_in_dnode = GetOfsInDnode(*path);
  block_t old_addr = (*dnode_page).GetPage<NodePage>().GetBlockAddr(ofs_in_dnode);
  // This page is already truncated
  if (old_addr == kNullAddr) {
    return kNullAddr;
  }
  // Check if IPU is allowed
  if (old_addr != kNewAddr && !page->IsColdData() &&
      fs_->GetSegmentManager().NeedInplaceUpdate(IsDir())) {
    return old_addr;
  }

  // Allocate a new addr
  NodeInfo ni;
  nid_t nid = (*dnode_page).GetPage<NodePage>().NidOfNode();
  fs_->GetNodeManager().GetNodeInfo(nid, ni);

  Summary sum;
  SetSummary(&sum, nid, ofs_in_dnode, ni.version);
  block_t new_addr =
      fs_->GetSegmentManager().GetBlockAddrOnSegment(page, old_addr, &sum, PageType::kData);
  ZX_DEBUG_ASSERT(new_addr != kNullAddr && new_addr != kNewAddr && new_addr != old_addr);

  (*dnode_page).WaitOnWriteback();
  (*dnode_page).GetPage<NodePage>().SetDataBlkaddr(ofs_in_dnode, new_addr);
  (*dnode_page).SetDirty();
  UpdateExtentCache(page->GetIndex(), new_addr);
  data_version_ = superblock_info_.GetCheckpointVer();
  return new_addr;
}

zx::result<std::vector<LockedPage>> VnodeF2fs::WriteBegin(const size_t offset, const size_t len) {
  const pgoff_t index_start = safemath::CheckDiv<pgoff_t>(offset, kBlockSize).ValueOrDie();
  const size_t offset_end = safemath::CheckAdd<size_t>(offset, len).ValueOrDie();
  const pgoff_t index_end = CheckedDivRoundUp<pgoff_t>(offset_end, kBlockSize);

  zx::result pages = GrabLockedPages(index_start, index_end);
  if (unlikely(pages.is_error())) {
    return pages.take_error();
  }

  for (auto &page : *pages) {
    page.WaitOnWriteback();
    page.SetDirty();
  }
  if (zx::result result = GetAddresses(index_start, index_end - index_start); result.is_error()) {
    for (auto &page : *pages) {
      page.Invalidate();
    }
    return result.take_error();
  }
  return zx::ok(*std::move(pages));
}

zx::result<std::vector<block_t>> VnodeF2fs::GetAddresses(const std::vector<pgoff_t> &indices) {
  std::vector<block_t> data_block_addresses(indices.size());
  size_t prev_node_offset = kInvalidNodeOffset;
  std::vector<pgoff_t> allocated;
  auto invalidate_addrs = fit::defer([&]() TA_NO_THREAD_SAFETY_ANALYSIS {
    for (pgoff_t addr : allocated) {
      TruncateHoleUnsafe(addr, addr + 1, false);
    }
  });

  LockedPage dnode_page;
  for (uint32_t iter = 0; iter < indices.size(); ++iter) {
    if (zx::result data_blkaddr = LookupExtentCacheBlock(indices[iter]); data_blkaddr.is_ok()) {
      ZX_DEBUG_ASSERT(data_blkaddr.value() != kNullAddr || data_blkaddr.value() != kNewAddr);
      data_block_addresses[iter] = data_blkaddr.value();
      continue;
    }

    zx::result path = GetNodePath(indices[iter]);
    if (path.is_error()) {
      return path.take_error();
    }
    if (!IsSameDnode(*path, prev_node_offset)) {
      dnode_page.reset();
      zx::result next_page = fs()->GetNodeManager().GetLockedDnodePage(*path, IsDir());
      if (next_page.is_error()) {
        return next_page.take_error();
      }
      AddBlocksUnsafe(path->num_new_nodes);
      dnode_page = *std::move(next_page);
      prev_node_offset = path->node_offset[path->depth];
    }

    size_t ofs_in_dnode = GetOfsInDnode(*path);
    block_t data_blkaddr = dnode_page.GetPage<NodePage>().GetBlockAddr(ofs_in_dnode);

    if (data_blkaddr == kNullAddr) {
      if (zx_status_t err = ReserveNewBlock(dnode_page, ofs_in_dnode); err != ZX_OK) {
        return zx::error(err);
      }
      AddBlocksUnsafe(1);
      data_blkaddr = kNewAddr;
      allocated.push_back(indices[iter]);
    }

    data_block_addresses[iter] = data_blkaddr;
  }
  invalidate_addrs.cancel();
  return zx::ok(std::move(data_block_addresses));
}

zx::result<std::vector<block_t>> VnodeF2fs::GetAddresses(pgoff_t index, size_t count) {
  std::vector<pgoff_t> indices(count);
  std::iota(indices.begin(), indices.end(), index);
  return GetAddresses(indices);
}

zx::result<std::vector<block_t>> VnodeF2fs::FindAddresses(const std::vector<pgoff_t> &indices) {
  std::vector<block_t> data_block_addresses(indices.size());
  size_t prev_node_offset = kInvalidNodeOffset;
  LockedPage dnode_page;

  for (uint32_t iter = 0; iter < indices.size(); ++iter) {
    if (zx::result data_blkaddr = LookupExtentCacheBlock(indices[iter]); data_blkaddr.is_ok()) {
      ZX_DEBUG_ASSERT(data_blkaddr.value() != kNullAddr || data_blkaddr.value() != kNewAddr);
      data_block_addresses[iter] = data_blkaddr.value();
      continue;
    }

    zx::result path = GetNodePath(indices[iter]);
    if (path.is_error()) {
      return path.take_error();
    }
    if (!IsSameDnode(*path, prev_node_offset)) {
      dnode_page.reset();
      zx::result next_page = fs()->GetNodeManager().FindLockedDnodePage(*path);
      if (next_page.is_error()) {
        if (next_page.error_value() == ZX_ERR_NOT_FOUND) {
          prev_node_offset = kInvalidNodeOffset;
          data_block_addresses[iter] = kNullAddr;
          continue;
        }
        return next_page.take_error();
      }
      dnode_page = *std::move(next_page);
      prev_node_offset = path->node_offset[path->depth];
    }
    ZX_DEBUG_ASSERT(dnode_page != nullptr);

    size_t ofs_in_dnode = GetOfsInDnode(*path);
    block_t data_blkaddr = dnode_page.GetPage<NodePage>().GetBlockAddr(ofs_in_dnode);
    data_block_addresses[iter] = data_blkaddr;
  }
  return zx::ok(std::move(data_block_addresses));
}

zx::result<std::vector<block_t>> VnodeF2fs::FindAddresses(pgoff_t index, size_t count) {
  std::vector<pgoff_t> indices(count);
  std::iota(indices.begin(), indices.end(), index);
  return FindAddresses(indices);
}

}  // namespace f2fs
