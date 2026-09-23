// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>

#include <numeric>
#include <vector>

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

constexpr uint32_t kMaxNodeCnt = 10;

class NodeManagerTest : public F2fsFakeDevTestFixture {
 public:
  NodeManagerTest() : F2fsFakeDevTestFixture(TestOptions{.run_fsck = false}) {}
};

zx_status_t GetLockedDnodePage(VnodeF2fs &vnode, pgoff_t index, LockedPage *out) {
  auto path_or = vnode.GetNodePath(index);
  if (path_or.is_error()) {
    return path_or.error_value();
  }
  auto page_or = vnode.fs()->GetNodeManager().GetLockedDnodePage(*path_or, vnode.IsDir());
  if (page_or.is_ok()) {
    *out = std::move(*page_or);
  }
  vnode.AddBlocks(path_or->num_new_nodes);
  return page_or.status_value();
}

// A file offset that maps to a level-2 (single-indirect) path:
// inode -> indirect (kNodeInd1Block) -> direct -> data.
constexpr pgoff_t kLevel2Index = kAddrsPerInode + kAddrsPerBlock * 2;

// The node path to kLevel2Index plus the real intermediate IndirectNode nid and its
// child DirectNode nid, as produced by BuildLevel2Tree().
struct Level2Tree {
  NodePath path;
  nid_t indirect_nid = 0;
  nid_t direct_nid = 0;
};

// Allocates a level-2 block map for |vnode| and fills |*out|, verifying the node types so
// that a caller's later corruption is meaningful. Wrap calls in ASSERT_NO_FATAL_FAILURE so
// an inner assertion failure propagates to the test.
void BuildLevel2Tree(VnodeF2fs &vnode, Level2Tree *out) {
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(vnode, kLevel2Index, &dnode_page), ZX_OK);
  }

  auto path_or = vnode.GetNodePath(kLevel2Index);
  ASSERT_TRUE(path_or.is_ok());
  ASSERT_EQ(path_or->depth, static_cast<size_t>(2));

  NodeManager &node_manager = vnode.fs()->GetNodeManager();
  {
    LockedPage ipage;
    ASSERT_EQ(node_manager.GetNodePage(vnode.Ino(), &ipage), ZX_OK);
    out->indirect_nid = ipage.GetPage<NodePage>().GetNid(path_or->offset_in_node[0]);
  }
  {
    LockedPage ind_page;
    ASSERT_EQ(node_manager.GetNodePage(out->indirect_nid, &ind_page), ZX_OK);
    ASSERT_FALSE(ind_page.GetPage<NodePage>().IsDnode());
    out->direct_nid = ind_page.GetPage<NodePage>().GetNid(path_or->offset_in_node[1]);
  }
  {
    LockedPage dir_page;
    ASSERT_EQ(node_manager.GetNodePage(out->direct_nid, &dir_page), ZX_OK);
    ASSERT_TRUE(dir_page.GetPage<NodePage>().IsDnode());
  }

  out->path = *path_or;
}

void FaultInjectToDnodeAndTruncate(NodeManager &node_manager, fbl::RefPtr<VnodeF2fs> &vnode,
                                   pgoff_t page_index, block_t fault_address,
                                   zx_status_t exception_type) TA_NO_THREAD_SAFETY_ANALYSIS {
  ino_t node_id;
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, page_index, &dnode_page), ZX_OK);
    node_id = dnode_page.GetPage<NodePage>().NidOfNode();
  }
  block_t temp_block_address;

  // Write out dirty nodes to allocate lba
  vnode->fs()->GetNodeVnode().Writeback(true, true);
  MapTester::GetCachedNatEntryBlockAddress(node_manager, node_id, temp_block_address);
  vnode->fs()->GetNodeVnode().InvalidatePages();

  // Set fault_address to the NAT entry
  MapTester::SetCachedNatEntryBlockAddress(node_manager, node_id, fault_address);

  ASSERT_EQ(vnode->TruncateInodeBlocks(page_index), exception_type);

  // Restore the NAT entry
  MapTester::SetCachedNatEntryBlockAddress(node_manager, node_id, temp_block_address);

  // Retry truncate
  vnode->fs()->GetNodeVnode().InvalidatePages();
  ASSERT_EQ(vnode->TruncateInodeBlocks(page_index), ZX_OK);
}

TEST_F(NodeManagerTest, NatCache) {
  NodeManager &node_manager = fs_->GetNodeManager();

  size_t num_tree = 0, num_clean = 0, num_dirty = 0;

  // 1. Check NAT cache is empty
  MapTester::GetNatCacheEntryCount(node_manager, num_tree, num_clean, num_dirty);
  ASSERT_EQ(num_tree, 1UL);
  ASSERT_EQ(num_clean, 1UL);  // root inode
  ASSERT_EQ(num_dirty, 0UL);

  // 2. Check NAT entry is cached in dirty NAT entries list
  std::vector<fbl::RefPtr<VnodeF2fs>> vnodes;
  std::vector<uint32_t> inos;

  // Fill NAT cache
  FileTester::CreateChildren(fs_.get(), vnodes, inos, root_dir_, "NATCache_", kMaxNodeCnt);
  ASSERT_EQ(vnodes.size(), kMaxNodeCnt);
  ASSERT_EQ(inos.size(), kMaxNodeCnt);

  MapTester::GetNatCacheEntryCount(node_manager, num_tree, num_clean, num_dirty);
  ASSERT_EQ(num_tree, static_cast<size_t>(kMaxNodeCnt + 1));
  ASSERT_EQ(num_clean, static_cast<size_t>(1));
  ASSERT_EQ(num_dirty, static_cast<size_t>(kMaxNodeCnt));
  ASSERT_EQ(node_manager.GetNatCount(), static_cast<nid_t>(kMaxNodeCnt + 1));

  // Lookup NAT cache
  for (auto ino : inos) {
    NodeInfo ni;
    ASSERT_TRUE(MapTester::IsCachedNat(node_manager, ino));
    fs_->GetNodeManager().GetNodeInfo(ino, ni);
    ASSERT_EQ(ni.nid, ino);
  }

  // Move dirty entries to clean entries
  fs_->SyncFs();

  // 3. Check NAT entry is cached in clean NAT entries list
  MapTester::GetNatCacheEntryCount(node_manager, num_tree, num_clean, num_dirty);
  ASSERT_EQ(num_tree, static_cast<size_t>(kMaxNodeCnt + 1));
  ASSERT_EQ(num_clean, static_cast<size_t>(kMaxNodeCnt + 1));
  ASSERT_EQ(num_dirty, static_cast<size_t>(0));
  ASSERT_EQ(node_manager.GetNatCount(), static_cast<nid_t>(kMaxNodeCnt + 1));

  // Lookup NAT cache
  for (auto ino : inos) {
    NodeInfo ni;
    ASSERT_TRUE(MapTester::IsCachedNat(node_manager, ino));
    fs_->GetNodeManager().GetNodeInfo(ino, ni);
    ASSERT_EQ(ni.nid, ino);
  }

  // 4. Flush all NAT cache entries
  MapTester::RemoveAllNatEntries(node_manager);
  ASSERT_EQ(node_manager.GetNatCount(), static_cast<uint32_t>(0));

  CursegInfo *curseg =
      fs_->GetSegmentManager().CURSEG_I(CursegType::kCursegHotData);  // NAT Journal
  SummaryBlock *sum = &curseg->sum_blk;
  ASSERT_EQ(GetSumType(&sum->footer), kSumTypeData);

  MapTester::GetNatCacheEntryCount(node_manager, num_tree, num_clean, num_dirty);
  ASSERT_EQ(num_tree, static_cast<size_t>(0));
  ASSERT_EQ(num_clean, static_cast<size_t>(0));
  ASSERT_EQ(num_dirty, static_cast<size_t>(0));
  ASSERT_EQ(NatsInCursum(*sum), static_cast<int>(kMaxNodeCnt + 1));

  // Lookup NAT journal
  for (auto ino : inos) {
    NodeInfo ni;
    ASSERT_FALSE(MapTester::IsCachedNat(node_manager, ino));
    fs_->GetNodeManager().GetNodeInfo(ino, ni);
    ASSERT_EQ(ni.nid, ino);
  }

  // 5. Check NAT cache miss and journal miss
  std::vector<uint32_t> journal_inos;

  // Fill NAT cache with journal size -2
  // Root inode NAT(nid=4) is duplicated in cache and journal, so we need to keep two empty NAT
  // entries
  FileTester::CreateChildren(fs_.get(), vnodes, journal_inos, root_dir_, "NATJournal_",
                             kNatJournalEntries - kMaxNodeCnt - 2);
  ASSERT_EQ(vnodes.size(), kNatJournalEntries - 2);
  ASSERT_EQ(inos.size() + journal_inos.size(), kNatJournalEntries - 2);

  // Fill NAT journal
  fs_->SyncFs();
  ASSERT_EQ(NatsInCursum(*sum), static_cast<int>(kNatJournalEntries - 1));

  // Fill NAT cache over journal size
  FileTester::CreateChildren(fs_.get(), vnodes, journal_inos, root_dir_, "NATJournalFlush_", 2);
  ASSERT_EQ(vnodes.size(), kNatJournalEntries);
  ASSERT_EQ(inos.size() + journal_inos.size(), kNatJournalEntries);

  // Flush NAT journal
  fs_->SyncFs();
  ASSERT_EQ(NatsInCursum(*sum), static_cast<int>(0));

  // Flush NAT cache
  MapTester::RemoveAllNatEntries(node_manager);
  ASSERT_EQ(node_manager.GetNatCount(), static_cast<uint32_t>(0));

  // Check NAT cache empty
  MapTester::GetNatCacheEntryCount(node_manager, num_tree, num_clean, num_dirty);
  ASSERT_EQ(num_tree, static_cast<size_t>(0));
  ASSERT_EQ(num_clean, static_cast<size_t>(0));
  ASSERT_EQ(num_dirty, static_cast<size_t>(0));
  ASSERT_EQ(node_manager.GetNatCount(), static_cast<nid_t>(0));

  // Read NAT block
  for (auto ino : inos) {
    NodeInfo ni;
    ASSERT_FALSE(MapTester::IsCachedNat(node_manager, ino));
    fs_->GetNodeManager().GetNodeInfo(ino, ni);
    ASSERT_EQ(ni.nid, ino);
  }

  MapTester::GetNatCacheEntryCount(node_manager, num_tree, num_clean, num_dirty);
  ASSERT_EQ(num_tree, static_cast<size_t>(10));
  ASSERT_EQ(num_clean, static_cast<size_t>(10));
  ASSERT_EQ(num_dirty, static_cast<size_t>(0));
  ASSERT_EQ(node_manager.GetNatCount(), static_cast<nid_t>(10));

  // Shrink nat cache to reduce memory usage (test TryToFreeNats())
  MapTester::SetNatCount(node_manager, node_manager.GetNatCount() + kNmWoutThreshold * 3);
  fs_->SyncFs();

  MapTester::GetNatCacheEntryCount(node_manager, num_tree, num_clean, num_dirty);
  ASSERT_EQ(num_tree, static_cast<size_t>(0));
  ASSERT_EQ(num_clean, static_cast<size_t>(0));
  ASSERT_EQ(node_manager.GetNatCount(), static_cast<uint32_t>(kNmWoutThreshold * 3));
  MapTester::SetNatCount(node_manager, 0);

  for (auto &vnode_refptr : vnodes) {
    ASSERT_EQ(vnode_refptr->Close(), ZX_OK);
    vnode_refptr.reset();
  }
}

TEST_F(NodeManagerTest, FreeNid) {
  NodeManager &node_manager = fs_->GetNodeManager();
  auto init_fcnt = node_manager.GetFreeNidCount();

  nid_t nid = MapTester::ScanFreeNidList(node_manager);
  // |nid| is the last element in free nid tree.
  ASSERT_EQ(nid + 1, node_manager.GetNextScanNid());

  auto nid_or = fs_->GetNodeManager().AllocNid();
  ASSERT_TRUE(nid_or.is_ok());
  ASSERT_EQ(nid_or, static_cast<nid_t>(4));
  ASSERT_EQ(node_manager.GetFreeNidCount(), init_fcnt - 1);

  nid = MapTester::GetNextFreeNidInList(node_manager);
  ASSERT_EQ(nid, static_cast<nid_t>(5));

  nid_or = fs_->GetNodeManager().AllocNid();
  ASSERT_TRUE(nid_or.is_ok());
  ASSERT_EQ(*nid_or, static_cast<nid_t>(5));
  ASSERT_EQ(node_manager.GetFreeNidCount(), init_fcnt - 2);

  fs_->GetNodeManager().AddFreeNid(*nid_or);
  nid = MapTester::GetNextFreeNidInList(node_manager);
  ASSERT_EQ(nid, static_cast<nid_t>(5));
}

TEST_F(NodeManagerTest, NodePage) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc Inode
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  NodeManager &node_manager = fs_->GetNodeManager();
  uint64_t free_node_cnt = node_manager.GetFreeNidCount();

  // Inode block
  //   |- direct node
  //   |- direct node
  //   |- indirect node
  //   |            `- direct node
  //   |- indirect node
  //   |            `- direct node
  //   `- double indirect node
  //                `- indirect node
  //                      `- direct node

  // Check inode (level 0)
  nid_t node_nid = vnode->Ino();
  const pgoff_t direct_index = 1;
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, direct_index, &dnode_page), ZX_OK);
    MapTester::CheckDnodePage(dnode_page.GetPage<NodePage>(), node_nid);
  }

  {
    auto path_or = vnode->GetNodePath(direct_index);
    ASSERT_TRUE(path_or.is_ok());
    auto dnode_page_or = fs_->GetNodeManager().FindLockedDnodePage(*path_or);
    ASSERT_TRUE(dnode_page_or.is_ok());
    MapTester::CheckDnodePage((*dnode_page_or).GetPage<NodePage>(), node_nid);
  }
  ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt);

  // Check direct node (level 1)
  node_nid += 1;
  const pgoff_t indirect_index_lv1 = direct_index + kAddrsPerInode;

  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1, &dnode_page), ZX_OK);
    MapTester::CheckDnodePage(dnode_page.GetPage<NodePage>(), node_nid);
  }

  {
    auto path_or = vnode->GetNodePath(indirect_index_lv1);
    ASSERT_TRUE(path_or.is_ok());
    auto dnode_page_or = fs_->GetNodeManager().FindLockedDnodePage(*path_or);
    ASSERT_TRUE(dnode_page_or.is_ok());
    MapTester::CheckDnodePage((*dnode_page_or).GetPage<NodePage>(), node_nid);
  }
  ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 1);

  // Check indirect node (level 2)
  node_nid += 2;
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t indirect_index_lv2 = indirect_index_lv1 + direct_blks * 2;

  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2, &dnode_page), ZX_OK);
    MapTester::CheckDnodePage(dnode_page.GetPage<NodePage>(), node_nid);
  }

  {
    auto path_or = vnode->GetNodePath(indirect_index_lv2);
    ASSERT_TRUE(path_or.is_ok());
    auto dnode_page_or = fs_->GetNodeManager().FindLockedDnodePage(*path_or);
    ASSERT_TRUE(dnode_page_or.is_ok());
    MapTester::CheckDnodePage((*dnode_page_or).GetPage<NodePage>(), node_nid);
  }
  ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 2);

  // Check second indirect node (level 2)
  node_nid += 2;
  const pgoff_t indirect_blks = static_cast<const pgoff_t>(kAddrsPerBlock) * kNidsPerBlock;

  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2 + indirect_blks, &dnode_page), ZX_OK);
    MapTester::CheckDnodePage(dnode_page.GetPage<NodePage>(), node_nid);
  }

  {
    auto path_or = vnode->GetNodePath(indirect_index_lv2 + indirect_blks);
    ASSERT_TRUE(path_or.is_ok());
    auto dnode_page_or = fs_->GetNodeManager().FindLockedDnodePage(*path_or);
    ASSERT_TRUE(dnode_page_or.is_ok());
    MapTester::CheckDnodePage((*dnode_page_or).GetPage<NodePage>(), node_nid);
  }
  ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 2);

  // Check double indirect node (level 3)
  node_nid += 3;
  pgoff_t indirect_index_lv3 = indirect_index_lv2 + indirect_blks * 2;

  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv3, &dnode_page), ZX_OK);
    MapTester::CheckDnodePage(dnode_page.GetPage<NodePage>(), node_nid);
  }

  {
    auto path_or = vnode->GetNodePath(indirect_index_lv3);
    ASSERT_TRUE(path_or.is_ok());
    auto dnode_page_or = fs_->GetNodeManager().FindLockedDnodePage(*path_or);
    ASSERT_TRUE(dnode_page_or.is_ok());
    MapTester::CheckDnodePage((*dnode_page_or).GetPage<NodePage>(), node_nid);
  }
  ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 3);

  vnode->SetBlockCount(0);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, NodePageExceptionCase) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc Inode
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  NodeManager &node_manager = fs_->GetNodeManager();
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

  // Inode block
  //   |- direct node
  //   |- direct node
  //   |- indirect node
  //   |            `- direct node
  //   |- indirect node
  //   |            `- direct node
  //   `- double indirect node
  //                `- indirect node
  //                      `- direct node

  // Check inode (level 0)
  const pgoff_t direct_index = 1;
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t indirect_blks = static_cast<const pgoff_t>(kAddrsPerBlock) * kNidsPerBlock;
  const pgoff_t indirect_index_lv1 = direct_index + kAddrsPerInode;
  const pgoff_t indirect_index_lv2 = indirect_index_lv1 + direct_blks * 2;
  const pgoff_t indirect_index_lv3 = indirect_index_lv2 + indirect_blks * 2;

  // Check invalid page offset exception case
  pgoff_t indirect_index_invalid_lv4 = indirect_index_lv3 + indirect_blks * kNidsPerBlock;
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_invalid_lv4, &dnode_page),
              ZX_ERR_NOT_FOUND);
  }

  // Check invalid address
  nid_t nid;
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv3 + 1, &dnode_page), ZX_OK);
    nid = dnode_page.GetPage<NodePage>().NidOfNode();
  }

  // fault injection for GetNodePage()
  fs_->SyncFs();

  MapTester::SetCachedNatEntryBlockAddress(node_manager, nid, kNullAddr);

  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv3, &dnode_page), ZX_ERR_NOT_FOUND);
  }

  // Check IncValidNodeCount() exception case
  block_t tmp_total_valid_block_count = superblock_info.GetValidBlockCount();
  superblock_info.SetValidBlockCount(superblock_info.GetTotalBlockCount());
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1 + direct_blks, &dnode_page),
              ZX_ERR_NO_SPACE);
  }
  superblock_info.SetValidBlockCount(tmp_total_valid_block_count);

  block_t tmp_total_valid_node_count = superblock_info.GetValidNodeCount();
  superblock_info.SetValidNodeCount(superblock_info.GetMaxNodeCount());
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1 + direct_blks, &dnode_page),
              ZX_ERR_NO_SPACE);
  }
  superblock_info.SetValidNodeCount(tmp_total_valid_node_count);

  // Check NewNodePage() exception case
  fbl::RefPtr<VnodeF2fs> test_vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, test_vnode);

  test_vnode->SetFlag(InodeInfoFlag::kNoAlloc);
  test_vnode->ClearDirty();
  ASSERT_EQ(test_vnode->NewInodePage().status_value(), ZX_ERR_ACCESS_DENIED);
  test_vnode->ClearFlag(InodeInfoFlag::kNoAlloc);

  tmp_total_valid_block_count = superblock_info.GetValidBlockCount();
  superblock_info.SetValidBlockCount(superblock_info.GetTotalBlockCount());
  ASSERT_EQ(test_vnode->NewInodePage().status_value(), ZX_ERR_NO_SPACE);
  ASSERT_EQ(test_vnode->Close(), ZX_OK);
  test_vnode.reset();
  superblock_info.SetValidBlockCount(tmp_total_valid_block_count);

  vnode->SetBlockCount(0);

  // Check MaxNid
  const Superblock &sb_raw = superblock_info.GetSuperblock();
  uint32_t nat_segs = LeToCpu(sb_raw.segment_count_nat) >> 1;
  uint32_t nat_blocks = nat_segs << LeToCpu(sb_raw.log_blocks_per_seg);
  ASSERT_EQ(fs_->GetNodeManager().GetMaxNid(), kNatEntryPerBlock * nat_blocks);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, TruncateDoubleIndirect) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc Inode
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

  // Inode block
  //   |- direct node
  //   |- direct node
  //   |- indirect node
  //   |            `- direct node
  //   |- indirect node
  //   |            `- direct node
  //   `- double indirect node
  //                `- indirect node
  //                      `- direct node

  // Alloc a double indirect node (level 3)
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t indirect_blks = static_cast<const pgoff_t>(kAddrsPerBlock) * kNidsPerBlock;
  const pgoff_t direct_index = kAddrsPerInode + 1;
  const pgoff_t indirect_index = direct_index + direct_blks * 2;
  const pgoff_t double_indirect_index = indirect_index + indirect_blks * 2;
  const uint32_t inode_cnt = 2;

  ASSERT_EQ(superblock_info.GetValidInodeCount(), inode_cnt);
  ASSERT_EQ(superblock_info.GetValidNodeCount(), inode_cnt);

  std::vector<nid_t> nids;
  NodeManager &node_manager = fs_->GetNodeManager();
  uint64_t initial_free_nid_cnt = node_manager.GetFreeNidCount();

  // Alloc a direct node at double_indirect_index
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, double_indirect_index, &dnode_page), ZX_OK);
    nids.push_back(dnode_page.GetPage<NodePage>().NidOfNode());
  }

  // # of alloc nodes = 1 double indirect + 1 indirect + 1 direct
  uint32_t alloc_node_cnt = 3;
  uint32_t node_cnt = inode_cnt + alloc_node_cnt;

  // alloc_dnode cnt should be one
  ASSERT_EQ(nids.size(), 1UL);
  ASSERT_EQ(superblock_info.GetValidInodeCount(), inode_cnt);
  ASSERT_EQ(superblock_info.GetValidNodeCount(), node_cnt);

  // Truncate double the indirect node
  ASSERT_EQ(vnode->TruncateInodeBlocks(double_indirect_index), ZX_OK);
  node_cnt = inode_cnt;
  ASSERT_EQ(superblock_info.GetValidNodeCount(), node_cnt);

  MapTester::RemoveTruncatedNode(node_manager, nids);
  ASSERT_EQ(nids.size(), 0UL);

  ASSERT_EQ(node_manager.GetFreeNidCount(), initial_free_nid_cnt - alloc_node_cnt);
  fs_->SyncFs();
  // After checkpoint, we can reuse the removed nodes
  ASSERT_EQ(node_manager.GetFreeNidCount(), initial_free_nid_cnt);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, TruncateIndirect) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc Inode
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

  // Inode block
  //   |- direct node
  //   |- direct node
  //   |- indirect node
  //   |            `- direct node
  // Fill indirect node (level 2)
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t direct_index = kAddrsPerInode + 1;
  const pgoff_t indirect_index = direct_index + direct_blks * 2;
  const uint32_t inode_cnt = 2;
  ASSERT_EQ(superblock_info.GetValidInodeCount(), inode_cnt);
  ASSERT_EQ(superblock_info.GetValidNodeCount(), inode_cnt);

  std::vector<nid_t> nids;
  NodeManager &node_manager = fs_->GetNodeManager();
  uint64_t initial_free_nid_cnt = node_manager.GetFreeNidCount();

  // Start from kAddrsPerInode to alloc new dnodes
  for (pgoff_t i = kAddrsPerInode; i <= indirect_index; i += kAddrsPerBlock) {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, i, &dnode_page), ZX_OK);
    nids.push_back(dnode_page.GetPage<NodePage>().NidOfNode());
  }

  uint32_t indirect_node_cnt = 1;
  uint32_t direct_node_cnt = 3;
  uint32_t node_cnt = inode_cnt + direct_node_cnt + indirect_node_cnt;
  uint32_t alloc_node_cnt = indirect_node_cnt + direct_node_cnt;

  ASSERT_EQ(nids.size(), direct_node_cnt);
  ASSERT_EQ(superblock_info.GetValidInodeCount(), inode_cnt);
  ASSERT_EQ(superblock_info.GetValidNodeCount(), node_cnt);

  // Truncate indirect nodes
  ASSERT_EQ(vnode->TruncateInodeBlocks(indirect_index), ZX_OK);
  --indirect_node_cnt;
  --direct_node_cnt;
  node_cnt = inode_cnt + direct_node_cnt + indirect_node_cnt;
  ASSERT_EQ(superblock_info.GetValidNodeCount(), node_cnt);

  MapTester::RemoveTruncatedNode(node_manager, nids);

  ASSERT_EQ(nids.size(), direct_node_cnt);

  // Truncate direct nodes
  ASSERT_EQ(vnode->TruncateInodeBlocks(direct_index), ZX_OK);
  direct_node_cnt -= 2;
  node_cnt = inode_cnt + direct_node_cnt + indirect_node_cnt;
  ASSERT_EQ(superblock_info.GetValidNodeCount(), node_cnt);

  MapTester::RemoveTruncatedNode(node_manager, nids);
  ASSERT_EQ(nids.size(), direct_node_cnt);

  ASSERT_EQ(superblock_info.GetValidInodeCount(), inode_cnt);

  ASSERT_EQ(node_manager.GetFreeNidCount(), initial_free_nid_cnt - alloc_node_cnt);
  fs_->SyncFs();
  // After checkpoint, we can reuse the removed nodes
  ASSERT_EQ(node_manager.GetFreeNidCount(), initial_free_nid_cnt);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, TruncateExceptionCase) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc Inode
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

  // Inode block
  //   |- direct node
  //   |- direct node

  // Fill direct node (level 1)
  const uint32_t inode_cnt = 2;

  ASSERT_EQ(superblock_info.GetValidInodeCount(), inode_cnt);
  ASSERT_EQ(superblock_info.GetValidNodeCount(), inode_cnt);

  const pgoff_t direct_index = 1;
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t indirect_blks = static_cast<const pgoff_t>(kAddrsPerBlock) * kNidsPerBlock;
  const pgoff_t indirect_index_lv1 = direct_index + kAddrsPerInode;
  const pgoff_t indirect_index_lv1_2nd = indirect_index_lv1 + direct_blks;
  const pgoff_t indirect_index_lv2 = indirect_index_lv1 + direct_blks * 2;
  const pgoff_t indirect_index_lv3 = indirect_index_lv2 + indirect_blks * 2;

  // Check invalid page offset exception case
  pgoff_t indirect_index_invalid_lv4 = indirect_index_lv3 + indirect_blks * kNidsPerBlock;

  std::vector<nid_t> nids;
  NodeManager &node_manager = fs_->GetNodeManager();
  uint64_t initial_free_nid_cnt = node_manager.GetFreeNidCount();

  // Start from kAddrsPerInode to alloc new dnodes
  for (pgoff_t i = kAddrsPerInode; i <= indirect_index_lv3 + kNidsPerBlock; i += kAddrsPerBlock) {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, i, &dnode_page), ZX_OK);
    nids.push_back(dnode_page.GetPage<NodePage>().NidOfNode());
  }

  uint32_t direct_node_cnt = 4 + kNidsPerBlock * 2;
  uint32_t indirect_node_cnt = 4;  // 1 double indirect + 3 indirect
  uint32_t node_cnt = inode_cnt + direct_node_cnt + indirect_node_cnt;

  ASSERT_EQ(nids.size(), direct_node_cnt);
  ASSERT_EQ(superblock_info.GetValidInodeCount(), inode_cnt);
  ASSERT_EQ(superblock_info.GetValidNodeCount(), node_cnt);

  // 1. Truncate invalid node
  ASSERT_EQ(vnode->TruncateInodeBlocks(indirect_index_invalid_lv4), ZX_ERR_NOT_FOUND);

  block_t fault_addr = kNewAddr - 1;
  // 2. Check exception case of TruncatePartialNodes()
  FaultInjectToDnodeAndTruncate(node_manager, vnode, indirect_index_lv3 + kNidsPerBlock, fault_addr,
                                ZX_ERR_OUT_OF_RANGE);
  FaultInjectToDnodeAndTruncate(node_manager, vnode, indirect_index_lv2 + kNidsPerBlock, fault_addr,
                                ZX_ERR_OUT_OF_RANGE);
  --indirect_node_cnt;

  // 3. Check exception case of TruncateNodes()
  FaultInjectToDnodeAndTruncate(node_manager, vnode, indirect_index_lv3, fault_addr,
                                ZX_ERR_OUT_OF_RANGE);
  FaultInjectToDnodeAndTruncate(node_manager, vnode, indirect_index_lv2, fault_addr,
                                ZX_ERR_OUT_OF_RANGE);
  --indirect_node_cnt;

  // 4. Check exception case of TruncateDnode()
  FaultInjectToDnodeAndTruncate(node_manager, vnode, indirect_index_lv1_2nd, fault_addr,
                                ZX_ERR_OUT_OF_RANGE);
  --indirect_node_cnt;

  // 5. Check exception case truncation of invalid address
  FaultInjectToDnodeAndTruncate(node_manager, vnode, indirect_index_lv1, kNullAddr, ZX_OK);
  --indirect_node_cnt;
  node_cnt = inode_cnt + indirect_node_cnt;
  ASSERT_EQ(superblock_info.GetValidNodeCount(), node_cnt);

  // 6. Wrap up
  MapTester::RemoveTruncatedNode(node_manager, nids);
  ASSERT_EQ(nids.size(), 0UL);

  ASSERT_EQ(superblock_info.GetValidInodeCount(), inode_cnt);

  fs_->SyncFs();

  // After checkpoint, we can reuse the removed nodes
  ASSERT_EQ(node_manager.GetFreeNidCount(), initial_free_nid_cnt);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, NodeFooter) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc Inode
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());
  nid_t inode_nid = vnode->Ino();

  {
    const pgoff_t direct_index = 1;
    LockedPage locked_dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, direct_index, &locked_dnode_page), ZX_OK);
    NodePage *dnode_page = &locked_dnode_page.GetPage<NodePage>();
    MapTester::CheckDnodePage(*dnode_page, inode_nid);

    LockedPage locked_page;
    fs_->GetNodeVnode().GrabLockedPage(direct_index, &locked_page);
    NodePage *page = &locked_page.GetPage<NodePage>();

    // Check CopyNodeFooterFrom()
    page->CopyNodeFooterFrom(*dnode_page);

    ASSERT_EQ(page->InoOfNode(), vnode->Ino());
    ASSERT_EQ(page->InoOfNode(), dnode_page->InoOfNode());
    ASSERT_EQ(page->NidOfNode(), dnode_page->NidOfNode());
    ASSERT_EQ(page->OfsOfNode(), dnode_page->OfsOfNode());
    ASSERT_EQ(page->CpverOfNode(), dnode_page->CpverOfNode());
    ASSERT_EQ(page->NextBlkaddrOfNode(), dnode_page->NextBlkaddrOfNode());

    // Check footer.flag
    ASSERT_EQ(page->IsFsyncDnode(), dnode_page->IsFsyncDnode());
    ASSERT_EQ(page->IsFsyncDnode(), false);
    page->SetFsyncMark(true);
    ASSERT_EQ(page->IsFsyncDnode(), true);
    page->SetFsyncMark(false);
    ASSERT_EQ(page->IsFsyncDnode(), false);

    ASSERT_EQ(page->IsDentDnode(), dnode_page->IsDentDnode());
    ASSERT_EQ(page->IsDentDnode(), false);
    page->SetDentryMark(false);
    ASSERT_EQ(page->IsDentDnode(), false);
    page->SetDentryMark(true);
    ASSERT_EQ(page->IsDentDnode(), true);
    bool mark = !fs_->GetNodeManager().IsCheckpointedNode(page->InoOfNode());
    page->SetDentryMark(mark);
    ASSERT_EQ(page->IsDentDnode(), true);

    MapTester::SetCachedNatEntryCheckpointed(fs_->GetNodeManager(), dnode_page->NidOfNode());
    mark = !fs_->GetNodeManager().IsCheckpointedNode(page->InoOfNode());
    page->SetDentryMark(mark);
    ASSERT_EQ(page->IsDentDnode(), false);
  }
  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, GetDataBlockAddressesSinglePage) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc inode
  fbl::RefPtr<VnodeF2fs> file_vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, file_vnode);
  auto vnode = fbl::RefPtr<File>::Downcast(std::move(file_vnode));
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  NodeManager &node_manager = fs_->GetNodeManager();
  uint64_t free_node_cnt = node_manager.GetFreeNidCount();
  uint8_t buf[kBlockSize] = {1};
  size_t out_actual;

  // Inode block
  //   |- direct node
  //   |- direct node
  //   |- indirect node
  //   |            `- direct node
  //   |- indirect node
  //   |            `- direct node
  //   `- double indirect node
  //                `- indirect node
  //                      `- direct node

  // Check inode (level 0)
  const pgoff_t direct_index = 0;
  pgoff_t file_offset = direct_index * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), direct_index, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(direct_index, 1);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt);

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, direct_index, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses.front());
  }

  // Check direct node (level 1)
  const pgoff_t indirect_index_lv1 = direct_index + kAddrsPerInode;
  file_offset = indirect_index_lv1 * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv1, 1);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 1);

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses.front());
  }

  // Check indirect node (level 2)
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t indirect_index_lv2 = indirect_index_lv1 + direct_blks * 2;
  file_offset = indirect_index_lv2 * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv2, 1);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 2);

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses.front());
  }

  // Check second indirect node (level 2)
  const pgoff_t indirect_blks = static_cast<const pgoff_t>(kAddrsPerBlock) * kNidsPerBlock;
  file_offset = (indirect_index_lv2 + indirect_blks) * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv2 + indirect_blks, 1);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 2);

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2 + indirect_blks, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses.front());
  }

  // Check double indirect node (level 3)
  pgoff_t indirect_index_lv3 = indirect_index_lv2 + indirect_blks * 2;
  file_offset = indirect_index_lv3 * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv3, 1);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 3);

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv3, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses.front());
  }

  vnode->SetBlockCount(0);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, GetDataBlockAddressesMultiPage) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc inode
  fbl::RefPtr<VnodeF2fs> file_vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, file_vnode);
  auto vnode = fbl::RefPtr<File>::Downcast(std::move(file_vnode));
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  NodeManager &node_manager = fs_->GetNodeManager();
  uint64_t free_node_cnt = node_manager.GetFreeNidCount();
  uint8_t buf[kBlockSize * 2] = {1};
  size_t out_actual;

  // Inode block
  //   |- direct node
  //   |- direct node
  //   |- indirect node
  //   |            `- direct node
  //   |- indirect node
  //   |            `- direct node
  //   `- double indirect node
  //                `- indirect node
  //                      `- direct node

  // Check inode (level 0)
  const pgoff_t direct_index = 0;
  pgoff_t file_offset = direct_index * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(direct_index, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt);  // inode dnode

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, direct_index, &dnode_page), ZX_OK);

    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[0]);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(1ULL), block_addresses[1]);
  }

  // Check direct node (level 1)
  const pgoff_t indirect_index_lv1 = direct_index + kAddrsPerInode;
  file_offset = indirect_index_lv1 * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv1, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 1);  // direct node

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1, &dnode_page), ZX_OK);

    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[0]);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(1ULL), block_addresses[1]);
  }

  // Check indirect node (level 2)
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t indirect_index_lv2 = indirect_index_lv1 + direct_blks * 2;
  file_offset = indirect_index_lv2 * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv2, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 2);  // indirect + direct node

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2, &dnode_page), ZX_OK);

    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[0]);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(1ULL), block_addresses[1]);
  }

  // Check second indirect node (level 2)
  const pgoff_t indirect_blks = static_cast<const pgoff_t>(kAddrsPerBlock) * kNidsPerBlock;
  file_offset = (indirect_index_lv2 + indirect_blks) * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv2 + indirect_blks, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 2);  // indirect + direct node

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2 + indirect_blks, &dnode_page), ZX_OK);

    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[0]);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(1ULL), block_addresses[1]);
  }

  // Check double indirect node (level 3)
  pgoff_t indirect_index_lv3 = indirect_index_lv2 + indirect_blks * 2;
  file_offset = indirect_index_lv3 * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv3, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(),
              free_node_cnt -= 3);  // double indirect + indirect + direct dnode

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv3, &dnode_page), ZX_OK);

    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[0]);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(1ULL), block_addresses[1]);
  }

  vnode->SetBlockCount(0);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, GetDataBlockAddressesCrossMultiPage) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Alloc inode
  fbl::RefPtr<VnodeF2fs> file_vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, file_vnode);
  auto vnode = fbl::RefPtr<File>::Downcast(std::move(file_vnode));
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  NodeManager &node_manager = fs_->GetNodeManager();
  uint64_t free_node_cnt = node_manager.GetFreeNidCount();
  uint8_t buf[kBlockSize * 2] = {1};
  size_t out_actual;

  // Inode block
  //   |- direct node
  //   |- direct node
  //   |- indirect node
  //   |            `- direct node
  //   |- indirect node
  //   |            `- direct node
  //   `- double indirect node
  //                `- indirect node
  //                      `- direct node

  // Check inode + direct node (level 0 ~ 1)
  pgoff_t file_offset = static_cast<pgoff_t>((kAddrsPerInode - 1)) * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(kAddrsPerInode - 1, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 1);  // direct node

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, kAddrsPerInode - 1, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(kAddrsPerInode - 1), block_addresses[0]);
    dnode_page.reset();

    ASSERT_EQ(GetLockedDnodePage(*vnode, kAddrsPerInode, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[1]);
  }

  // Check direct node + direct node (level 1)
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t indirect_index_lv1 = kAddrsPerInode;
  file_offset = (indirect_index_lv1 + direct_blks - 1) * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv1 + direct_blks - 1, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 1);  // direct dnode

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1 + direct_blks - 1, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(kAddrsPerBlock - 1), block_addresses[0]);
    dnode_page.reset();

    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1 + direct_blks, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[1]);
  }

  // Check direct node + indirect node (level 1 ~ 2)
  file_offset = (indirect_index_lv1 + direct_blks * 2 - 1) * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv1 + direct_blks * 2 - 1, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 2);  // indirect + direct node

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1 + direct_blks * 2 - 1, &dnode_page),
              ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(kAddrsPerBlock - 1), block_addresses[0]);
    dnode_page.reset();

    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv1 + direct_blks * 2, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[1]);
  }

  // Check indirect node (level 2)
  const pgoff_t indirect_index_lv2 = indirect_index_lv1 + direct_blks * kAddrsPerBlock;
  file_offset = (indirect_index_lv2 - 1) * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv2 - 1, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(), free_node_cnt -= 2);  //  direct node

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2 - 1, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(kAddrsPerBlock - 1), block_addresses[0]);
    dnode_page.reset();

    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[1]);
  }

  // Check second indirect node (level 3)
  const pgoff_t indirect_blks = static_cast<const pgoff_t>(kAddrsPerBlock) * kNidsPerBlock;
  file_offset = (indirect_index_lv2 + indirect_blks + direct_blks - 1) * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or =
        vnode->GetAddresses(indirect_index_lv2 + indirect_blks + direct_blks - 1, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(),
              free_node_cnt -= 3);  // direct node + indirect + direct node

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv2 + indirect_blks + direct_blks - 1,
                                 &dnode_page),
              ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(kAddrsPerBlock - 1), block_addresses[0]);
    dnode_page.reset();

    ASSERT_EQ(
        GetLockedDnodePage(*vnode, indirect_index_lv2 + indirect_blks + direct_blks, &dnode_page),
        ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[1]);
  }

  // Check double indirect node (level 2 ~ 3)
  pgoff_t indirect_index_lv3 = indirect_index_lv2 + indirect_blks * 2;
  file_offset = (indirect_index_lv3 + direct_blks - 1) * kBlockSize;
  {
    vnode->Truncate(file_offset);
    ASSERT_EQ(FileTester::Write(vnode.get(), buf, sizeof(buf), file_offset, &out_actual), ZX_OK);
    ASSERT_EQ(vnode->SyncFile(false), ZX_OK);

    auto block_addresses_or = vnode->GetAddresses(indirect_index_lv3 + direct_blks - 1, 2);
    ASSERT_TRUE(block_addresses_or.is_ok());
    auto block_addresses = block_addresses_or.value();
    ASSERT_EQ(block_addresses.size(), 2u);
    ASSERT_EQ(node_manager.GetFreeNidCount(),
              free_node_cnt -= 4);  // direct node + double indirect + indirect + direct node

    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv3 + direct_blks - 1, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(kAddrsPerBlock - 1), block_addresses[0]);
    dnode_page.reset();

    ASSERT_EQ(GetLockedDnodePage(*vnode, indirect_index_lv3 + direct_blks, &dnode_page), ZX_OK);
    ASSERT_EQ(dnode_page.GetPage<NodePage>().GetBlockAddr(0ULL), block_addresses[1]);
  }

  vnode->SetBlockCount(0);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, DnodeBidxConsistency) {
  // To test |StartBidxOfNode|, kTargetOffset must be bigger than kAddrsPerInode, which is 923.
  constexpr pgoff_t kTargetOffset = 30000U;
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<f2fs::File> vn = fbl::RefPtr<f2fs::File>::Downcast(*std::move(test_file));

  auto path_or = vn->GetNodePath(kTargetOffset);
  ASSERT_TRUE(path_or.is_ok());
  auto ofs_in_node_or = GetOfsInDnode(*path_or);

  size_t start_bidx_of_node;
  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vn, kTargetOffset, &dnode_page), ZX_OK);
    start_bidx_of_node = dnode_page.GetPage<NodePage>().StartBidxOfNode(vn->GetAddrsPerInode());
  }
  ASSERT_EQ(start_bidx_of_node + ofs_in_node_or, kTargetOffset);

  vn->Close();
  vn = nullptr;
}

TEST_F(NodeManagerTest, MalformedNodeTree) TA_NO_THREAD_SAFETY_ANALYSIS {
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  fbl::RefPtr<VnodeF2fs> vnode2;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode2);
  ASSERT_TRUE(vnode2->NewInodePage().is_ok());

  const pgoff_t direct_index = kAddrsPerInode;
  LockedPage dnode_page;
  ASSERT_EQ(GetLockedDnodePage(*vnode, direct_index, &dnode_page), ZX_OK);

  // Corrupt the inode's first direct node pointer (kNodeDir1Block) to point to the other inode NID
  // instead of the direct node NID.
  {
    LockedPage ipage;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(vnode->Ino(), &ipage), ZX_OK);
    ipage.WaitOnWriteback();
    ipage.GetPage<NodePage>().SetNid(kNodeDir1Block, vnode2->Ino());
    ipage.SetDirty();
  }

  // Attempting to find or get the direct node page should fail with ZX_ERR_NOT_FOUND
  // because the node at level 1 is an Inode, not a direct node.
  auto path_or = vnode->GetNodePath(direct_index);
  ASSERT_TRUE(path_or.is_ok());
  ASSERT_EQ(fs_->GetNodeManager().FindLockedDnodePage(*path_or).status_value(), ZX_ERR_NOT_FOUND);
  ASSERT_EQ(fs_->GetNodeManager().GetLockedDnodePage(*path_or, false).status_value(),
            ZX_ERR_NOT_FOUND);

  // Attempting to truncate should safely succeed by treating the invalid node as already removed,
  // without out-of-bounds memory access.
  ASSERT_EQ(vnode->TruncateInodeBlocks(direct_index), ZX_OK);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
  ASSERT_EQ(vnode2->Close(), ZX_OK);
  vnode2.reset();
}

TEST_F(NodeManagerTest, MalformedNodeTreeAtIndirectLevel) TA_NO_THREAD_SAFETY_ANALYSIS {
  // The intermediate-node type check in Find/GetLockedDnodePage (the
  // `i < level - 1 && IsDnode()` clause: an intermediate node must be an IndirectNode,
  // not a DirectNode) fires only at depth >= 2, so a level-1 path does not reach it.
  // This test builds a level-2 (single-indirect) path and points the intermediate slot
  // at a *real* DirectNode, so dropping that clause in production makes these
  // assertions fail.
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  Level2Tree tree;
  ASSERT_NO_FATAL_FAILURE(BuildLevel2Tree(*vnode, &tree));

  // Corrupt the intermediate slot (kNodeInd1Block) to point at the DirectNode instead
  // of the IndirectNode: the node at level < depth-1 is now a Dnode. (direct_nid is a
  // real leaf of this same tree, so no cycle is introduced.)
  {
    LockedPage ipage;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(vnode->Ino(), &ipage), ZX_OK);
    ipage.WaitOnWriteback();
    ipage.GetPage<NodePage>().SetNid(kNodeInd1Block, tree.direct_nid);
    ipage.SetDirty();
  }

  ASSERT_EQ(fs_->GetNodeManager().FindLockedDnodePage(tree.path).status_value(), ZX_ERR_NOT_FOUND);
  ASSERT_EQ(fs_->GetNodeManager().GetLockedDnodePage(tree.path, false).status_value(),
            ZX_ERR_NOT_FOUND);

  // Restore the intermediate pointer so the tree is well-formed for truncation/teardown.
  {
    LockedPage ipage;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(vnode->Ino(), &ipage), ZX_OK);
    ipage.WaitOnWriteback();
    ipage.GetPage<NodePage>().SetNid(kNodeInd1Block, tree.indirect_nid);
    ipage.SetDirty();
  }

  ASSERT_EQ(vnode->TruncateInodeBlocks(kLevel2Index), ZX_OK);
  vnode->SetBlockCount(0);
  fs_->SyncFs();
  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, MalformedNodeTreeLeafIsIndirect) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Complements MalformedNodeTreeAtIndirectLevel by exercising the leaf clause
  // `i == level - 1 && !IsDnode()` (a leaf of the path must be a DirectNode). The
  // wrong-typed node here is *another file's* IndirectNode, so no cycle/self-reference
  // is introduced -- pointing the leaf at its own parent would instead deadlock on
  // NodePage re-lock, which is a separate defect not under test here.
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  fbl::RefPtr<VnodeF2fs> vnode2;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode2);
  ASSERT_TRUE(vnode2->NewInodePage().is_ok());

  Level2Tree tree, foreign;
  ASSERT_NO_FATAL_FAILURE(BuildLevel2Tree(*vnode, &tree));
  ASSERT_NO_FATAL_FAILURE(BuildLevel2Tree(*vnode2, &foreign));
  ASSERT_NE(foreign.indirect_nid, tree.indirect_nid);

  // Corrupt the leaf slot to point at another file's IndirectNode where a DirectNode is
  // expected: the node at level == depth - 1 is now a non-Dnode. (BuildLevel2Tree has
  // already verified foreign.indirect_nid is an IndirectNode.)
  {
    LockedPage ind_page;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(tree.indirect_nid, &ind_page), ZX_OK);
    ind_page.WaitOnWriteback();
    ind_page.GetPage<NodePage>().SetNid(tree.path.offset_in_node[1], foreign.indirect_nid);
    ind_page.SetDirty();
  }

  ASSERT_EQ(fs_->GetNodeManager().FindLockedDnodePage(tree.path).status_value(), ZX_ERR_NOT_FOUND);
  ASSERT_EQ(fs_->GetNodeManager().GetLockedDnodePage(tree.path, false).status_value(),
            ZX_ERR_NOT_FOUND);

  // Restore the leaf pointer so both trees are well-formed for teardown.
  {
    LockedPage ind_page;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(tree.indirect_nid, &ind_page), ZX_OK);
    ind_page.WaitOnWriteback();
    ind_page.GetPage<NodePage>().SetNid(tree.path.offset_in_node[1], tree.direct_nid);
    ind_page.SetDirty();
  }

  ASSERT_EQ(vnode->TruncateInodeBlocks(kLevel2Index), ZX_OK);
  vnode->SetBlockCount(0);
  ASSERT_EQ(vnode2->TruncateInodeBlocks(kLevel2Index), ZX_OK);
  vnode2->SetBlockCount(0);
  fs_->SyncFs();
  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
  ASSERT_EQ(vnode2->Close(), ZX_OK);
  vnode2.reset();
}

TEST_F(NodeManagerTest, DnodeCacheKeyedOnRequestedOffset) TA_NO_THREAD_SAFETY_ANALYSIS {
  // FindAddresses() reuses the dnode it holds while consecutive indices resolve to the same
  // node, skipping the traversal and its node-type checks. Both sides of that comparison
  // must come from NodePath: keyed on the page's on-disk OfsOfNode() instead, a forged
  // footer collides with a deeper level's offset, and the inode page returned by the
  // depth-0 lookup gets reused where the level-1 direct node belongs.
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  // Index 0 lives in the inode's i_addr[]. kLevel1Index lives in the first direct node at
  // in-node offset kAddrsPerInode, which is one past the end of i_addr[] -- i.e. i_nid[0].
  constexpr pgoff_t kLevel1Index = static_cast<pgoff_t>(kAddrsPerInode) * 2;
  const std::vector<pgoff_t> indices = {0, kLevel1Index};

  zx::result allocated = vnode->GetAddresses(indices);
  ASSERT_TRUE(allocated.is_ok());
  const block_t level1_addr = (*allocated)[1];

  block_t forged_reuse_result;
  {
    LockedPage ipage;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(vnode->Ino(), &ipage), ZX_OK);
    // What a forged reuse would read instead of level1_addr.
    forged_reuse_result = ipage.GetPage<NodePage>().GetNid(kNodeDir1Block);

    // Claim the first direct node's offset, so a cache keyed on the on-disk value would
    // report this inode as the node the level-1 path expects.
    ipage.WaitOnWriteback();
    ipage->GetAddress<Node>()->footer.flag =
        CpuToLe(uint32_t{1} << static_cast<int>(BitShift::kOffsetBitShift));
    ipage.SetDirty();
    ASSERT_EQ(ipage.GetPage<NodePage>().OfsOfNode(), uint32_t{1});
    ASSERT_TRUE(ipage.GetPage<NodePage>().IsInode());
  }
  ASSERT_NE(forged_reuse_result, level1_addr);

  // Both indices in one request, so index 0 seeds the reuse state for kLevel1Index.
  zx::result found = vnode->FindAddresses(indices);
  ASSERT_TRUE(found.is_ok());
  EXPECT_EQ((*found)[1], level1_addr);
  EXPECT_NE((*found)[1], forged_reuse_result);

  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, NodeFooterMustMatchHowTheNodeWasFound) TA_NO_THREAD_SAFETY_ANALYSIS {
  // NodePage reads a block through its footer: IsInode() compares the recorded nid and ino,
  // and the accessors pick which arm of the union to use from that answer. A block whose
  // footer disagrees with the NAT entry it was found through therefore describes a layout
  // that is not its own, so it must not reach any of that.
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  constexpr pgoff_t kLevel1Index = kAddrsPerInode;
  zx::result allocated = vnode->GetAddresses({0, kLevel1Index});
  ASSERT_TRUE(allocated.is_ok());

  NodeManager &node_manager = fs_->GetNodeManager();
  const nid_t ino = vnode->Ino();

  // A footer naming a different node than the one that was asked for.
  {
    LockedPage ipage;
    ASSERT_EQ(node_manager.GetNodePage(ino, &ipage), ZX_OK);
    ipage.WaitOnWriteback();
    ipage->GetAddress<Node>()->footer.nid = CpuToLe(ino + 1);
    ipage.SetDirty();
  }
  {
    LockedPage ipage;
    EXPECT_EQ(node_manager.GetNodePage(ino, &ipage), ZX_ERR_IO_DATA_INTEGRITY);
  }

  // A footer naming an owner the NAT entry does not agree with.
  {
    LockedPage ipage;
    ASSERT_EQ(fs_->GetNodeVnode().GrabLockedPage(ino, &ipage), ZX_OK);
    ipage.WaitOnWriteback();
    Node &node = *ipage->GetAddress<Node>();
    node.footer.nid = CpuToLe(ino);
    node.footer.ino = CpuToLe(ino + 1);
    ipage.SetDirty();
  }
  {
    LockedPage ipage;
    EXPECT_EQ(node_manager.GetNodePage(ino, &ipage), ZX_ERR_IO_DATA_INTEGRITY);
  }

  // Restored, the inode is reachable again.
  {
    LockedPage ipage;
    ASSERT_EQ(fs_->GetNodeVnode().GrabLockedPage(ino, &ipage), ZX_OK);
    ipage.WaitOnWriteback();
    Node &node = *ipage->GetAddress<Node>();
    node.footer.nid = CpuToLe(ino);
    node.footer.ino = CpuToLe(ino);
    ipage.SetDirty();
  }
  {
    LockedPage ipage;
    EXPECT_EQ(node_manager.GetNodePage(ino, &ipage), ZX_OK);
  }

  ASSERT_EQ(vnode->TruncateInodeBlocks(kLevel1Index), ZX_OK);
  vnode->SetBlockCount(0);
  fs_->SyncFs();
  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, BlockAddrArrayStaysWithinTheNodeBlock) TA_NO_THREAD_SAFETY_ANALYSIS {
  // An inode's block address array starts at an offset |i_extra_isize| names, so an image can
  // place that start beyond the array. Recovery and GC index node pages directly, without a
  // vnode to reject the inode first, so the page itself must not hand out addresses it does
  // not hold.
  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  LockedPage ipage;
  ASSERT_EQ(fs_->GetNodeManager().GetNodePage(vnode->Ino(), &ipage), ZX_OK);
  ipage.WaitOnWriteback();
  NodePage &node_page = ipage.GetPage<NodePage>();
  Inode &inode = ipage->GetAddress<Node>()->i;

  // i_nid[] follows i_addr[], so the first entry past the array is i_nid[0]. Put a value there
  // that no bounded access can report, otherwise an unbounded one reads the zero that slot
  // holds and is indistinguishable from the hole a bounded one reports.
  constexpr block_t kPastTheArray = 0x5a5a5a5a;
  inode.i_nid[0] = CpuToLe(kPastTheArray);

  // A start past the end of i_addr[] leaves nothing to read or write.
  inode.i_inline |= kExtraAttr;
  inode.i_extra_isize = CpuToLe(uint16_t{kAddrsPerInode * sizeof(uint32_t)});
  EXPECT_EQ(node_page.GetBlockAddr(0), kNullAddr);
  node_page.SetDataBlkaddr(0, kNewAddr);
  EXPECT_EQ(node_page.GetBlockAddr(0), kNullAddr);
  EXPECT_EQ(LeToCpu(inode.i_nid[0]), kPastTheArray);

  // A start within the array leaves only the entries after it.
  constexpr size_t kStart = kMaxExtraAttrSize / sizeof(uint32_t);
  inode.i_extra_isize = CpuToLe(uint16_t{kMaxExtraAttrSize});
  const size_t remaining = kAddrsPerInode - kStart;
  EXPECT_EQ(node_page.GetBlockAddr(remaining), kNullAddr);
  node_page.SetDataBlkaddr(remaining, kNewAddr);
  EXPECT_EQ(node_page.GetBlockAddr(remaining), kNullAddr);
  EXPECT_EQ(LeToCpu(inode.i_nid[0]), kPastTheArray);

  // The last entry the array does hold is still reachable.
  node_page.SetDataBlkaddr(remaining - 1, kNewAddr);
  EXPECT_EQ(node_page.GetBlockAddr(remaining - 1), kNewAddr);
  EXPECT_EQ(LeToCpu(inode.i_addr[kAddrsPerInode - 1]), kNewAddr);

  inode.i_inline &= ~kExtraAttr;
  inode.i_addr[kAddrsPerInode - 1] = 0;
  inode.i_nid[0] = 0;
  ipage.reset();
  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

TEST_F(NodeManagerTest, IsDnodeDoubleIndirectSubtree) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Test node classification in double-indirect subtrees (files > ~8MB).
  // In double-indirect subtrees, nodes appear in repeating groups of (kNidsPerBlock + 1):
  // 1 Indirect Node followed by kNidsPerBlock Direct Nodes.
  constexpr uint32_t kOfsDoubleIndirectNode = 5 + 2 * kNidsPerBlock;

  fbl::RefPtr<VnodeF2fs> vnode;
  FileTester::VnodeWithoutParent(fs_.get(), S_IFREG, vnode);
  ASSERT_TRUE(vnode->NewInodePage().is_ok());

  // Allocate nodes in the double-indirect subtree (level 3).
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t indirect_blks = static_cast<const pgoff_t>(kAddrsPerBlock) * kNidsPerBlock;
  const pgoff_t double_indirect_index = kAddrsPerInode + direct_blks * 2 + indirect_blks * 2;

  {
    LockedPage dnode_page;
    ASSERT_EQ(GetLockedDnodePage(*vnode, double_indirect_index, &dnode_page), ZX_OK);

    // 1. Verify the leaf Direct Node at level 3 (offset 2043 = kOfsDoubleIndirectNode + 2).
    // Without the fix in NodePage::IsDnode(), IsDnode() erroneously returned false.
    EXPECT_TRUE(dnode_page.GetPage<NodePage>().IsDnode())
        << "Leaf node in double-indirect subtree must be classified as "
           "a Direct Node (Dnode).";
    EXPECT_EQ(dnode_page.GetPage<NodePage>().OfsOfNode(), kOfsDoubleIndirectNode + 2);
    EXPECT_EQ(dnode_page.GetPage<NodePage>().StartBidxOfNode(vnode->GetAddrsPerInode()),
              double_indirect_index);
  }

  // 2. Verify the intermediate Indirect Node at level 2 (offset 2042 = kOfsDoubleIndirectNode + 1).
  // Without the fix in NodePage::IsDnode(), IsDnode() erroneously returned true.
  auto path_or = vnode->GetNodePath(double_indirect_index);
  ASSERT_TRUE(path_or.is_ok());

  nid_t ind_nid;
  {
    LockedPage ipage;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(vnode->Ino(), &ipage), ZX_OK);
    nid_t dind_nid = ipage.GetPage<NodePage>().GetNid(path_or->offset_in_node[0]);

    LockedPage dind_page;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(dind_nid, &dind_page), ZX_OK);
    EXPECT_FALSE(dind_page.GetPage<NodePage>().IsDnode())
        << "Double indirect node must not be classified as a Direct Node (Dnode).";
    EXPECT_EQ(dind_page.GetPage<NodePage>().OfsOfNode(), kOfsDoubleIndirectNode);
    ind_nid = dind_page.GetPage<NodePage>().GetNid(path_or->offset_in_node[1]);
  }

  {
    LockedPage ind_page;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(ind_nid, &ind_page), ZX_OK);
    EXPECT_FALSE(ind_page.GetPage<NodePage>().IsDnode())
        << "Intermediate node in double-indirect subtree must not be "
           "classified as a Direct Node (Dnode).";
    EXPECT_EQ(ind_page.GetPage<NodePage>().OfsOfNode(), kOfsDoubleIndirectNode + 1);
  }

  // Clean up allocated nodes so vnode teardown succeeds cleanly.
  ASSERT_EQ(vnode->TruncateInodeBlocks(double_indirect_index), ZX_OK);
  vnode->SetBlockCount(0);
  fs_->SyncFs();
  ASSERT_EQ(vnode->Close(), ZX_OK);
  vnode.reset();
}

}  // namespace
}  // namespace f2fs
