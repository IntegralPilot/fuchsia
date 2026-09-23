// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_NODE_H_
#define SRC_STORAGE_F2FS_NODE_H_

#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_wavl_tree.h>

#include "src/storage/f2fs/bitmap.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/layout.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

class F2fs;
class SuperblockInfo;
class NodePage;
class LockedPage;

// start node id of a node block dedicated to the given node id
inline uint32_t StartNid(uint32_t nid) { return (nid / kNatEntryPerBlock) * kNatEntryPerBlock; }

// node block offset on the NAT area dedicated to the given start node id
inline uint64_t NatBlockOffset(uint32_t start_nid) { return start_nid / kNatEntryPerBlock; }

// # of pages to perform readahead before building free nids
constexpr int kFreeNidPages = 4;

// maximum # of free node ids to produce during build_free_nids
constexpr int kMaxFreeNids = kNatEntryPerBlock * kFreeNidPages;

// maximum readahead size for node during getting data blocks
constexpr int kMaxRaNode = 128;

// maximum cached nat entries to manage memory footprint
constexpr uint32_t kNmWoutThreshold = 64 * kNatEntryPerBlock;

// vector size for gang look-up from nat cache that consists of radix tree
constexpr uint32_t kNatvecSize = 64;

// For directory operation
constexpr size_t kNodeDir1Block = kAddrsPerInode + 1;
constexpr size_t kNodeDir2Block = kAddrsPerInode + 2;
constexpr size_t kNodeInd1Block = kAddrsPerInode + 3;
constexpr size_t kNodeInd2Block = kAddrsPerInode + 4;
constexpr size_t kNodeDIndBlock = kAddrsPerInode + 5;

// maximum node block level of data block
constexpr uint32_t kMaxNodeBlockLevel = 4;

// For node information
struct NodeInfo {
  nid_t nid = 0;         // node id
  nid_t ino = 0;         // inode number of the node's owner
  block_t blk_addr = 0;  // block address of the node
  uint8_t version = 0;   // version of the node
};

constexpr size_t kMaxNodeDepth = 4;
struct NodePath {
  nid_t ino = 0;
  block_t num_new_nodes = 0;
  size_t depth = 0;
  size_t node_offset[kMaxNodeDepth] = {
      0,
  };
  size_t offset_in_node[kMaxNodeDepth] = {
      0,
  };
};

inline size_t GetOfsInDnode(NodePath &path) { return path.offset_in_node[path.depth]; }

class NatEntry : public fbl::WAVLTreeContainable<std::unique_ptr<NatEntry>>,
                 public fbl::DoublyLinkedListable<NatEntry *> {
 public:
  NatEntry() = default;
  NatEntry(const NatEntry &) = delete;
  NatEntry &operator=(const NatEntry &) = delete;
  NatEntry(const NatEntry &&) = delete;
  NatEntry &operator=(NatEntry &&) = delete;

  const NodeInfo &GetNodeInfo() { return ni_; }
  void SetNodeInfo(const NodeInfo &value) { ni_ = value; }

  bool IsCheckpointed() const { return checkpointed_; }
  void SetCheckpointed() { checkpointed_ = true; }
  void ClearCheckpointed() { checkpointed_ = false; }
  uint32_t GetNid() const { return ni_.nid; }
  void SetNid(const nid_t value) { ni_.nid = value; }
  block_t GetBlockAddress() const { return ni_.blk_addr; }
  void SetBlockAddress(const block_t value) { ni_.blk_addr = value; }
  uint32_t GetIno() const { return ni_.ino; }
  void SetIno(const nid_t value) { ni_.ino = value; }
  uint8_t GetVersion() const { return ni_.version; }
  void SetVersion(const uint8_t value) { ni_.version = value; }
  ino_t GetKey() const { return ni_.nid; }

 private:
  bool checkpointed_ = false;  // whether it is checkpointed or not
  NodeInfo ni_;                // in-memory node information
};

class F2fsFakeDevTestFixture;

class NodeManager {
 public:
  // Not copyable or moveable
  NodeManager(const NodeManager &) = delete;
  NodeManager &operator=(const NodeManager &) = delete;
  NodeManager(NodeManager &&) = delete;
  NodeManager &operator=(NodeManager &&) = delete;

  explicit NodeManager(F2fs *fs);
  ~NodeManager();

  zx_status_t BuildNodeManager();
  zx_status_t GetNodePage(nid_t nid, LockedPage *out);
  zx::result<std::vector<LockedPage>> GetNodePages(const std::vector<nid_t> &nids);

  // If it fails to find a node at |path|, it creates and returns a new node.
  zx::result<LockedPage> GetLockedDnodePage(NodePath &path, bool is_dir);
  // It returns a node page if it succeeds to find one at |path|.
  zx::result<LockedPage> FindLockedDnodePage(NodePath &path);

  zx_status_t GetNodeInfo(nid_t nid, NodeInfo &out);
  void SetNodeAddr(NodeInfo &ni, block_t new_blkaddr);

  pgoff_t FsyncNodePages(nid_t ino) __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());

  bool IsCheckpointedNode(nid_t nid);

  bool FlushNatsInJournal();
  zx_status_t FlushNatEntries();

  zx_status_t RecoverInodePage(NodePage &page);

  // Check whether the given nid is within node id range.
  zx_status_t CheckNidRange(const nid_t &nid) const { return nid < max_nid_; }

  zx::result<nid_t> AllocNid() __TA_EXCLUDES(free_nid_tree_lock_);
  zx::result<nid_t> GetNextFreeNid() __TA_EXCLUDES(free_nid_tree_lock_);
  int AddFreeNid(nid_t nid) __TA_EXCLUDES(free_nid_tree_lock_);

  void TruncateNode(nid_t nid);
  zx::result<LockedPage> NewNodePage(nid_t ino, nid_t nid, bool is_dirt, size_t ofs);

  // for fsck and tests
  explicit NodeManager(SuperblockInfo *sb);
  void SetMaxNid(const nid_t value) { max_nid_ = value; }
  nid_t GetMaxNid() const { return max_nid_; }
  void SetNatAddress(const block_t value) { nat_blkaddr_ = value; }
  block_t GetNatAddress() const { return nat_blkaddr_; }
  void SetNextScanNid(const nid_t value) __TA_EXCLUDES(build_lock_) {
    std::lock_guard lock(build_lock_);
    next_scan_nid_ = value;
  }
  nid_t GetNextScanNid() __TA_EXCLUDES(build_lock_) {
    std::lock_guard lock(build_lock_);
    return next_scan_nid_;
  }
  nid_t GetNatCount() const { return nat_entries_count_; }
  zx_status_t AllocNatBitmap(const uint32_t size) {
    nat_bitmap_size_ = size;
    nat_bitmap_.Reset(GetBitSize(nat_bitmap_size_));
    return ZX_OK;
  }
  void SetNatBitmap(const uint8_t *bitmap) {
    CloneBits(nat_bitmap_, bitmap, 0, GetBitSize(nat_bitmap_size_));
  }
  RawBitmap &GetNatBitmap() { return nat_bitmap_; }
  void GetNatBitmap(void *out);
  size_t GetFreeNidCount() __TA_EXCLUDES(free_nid_tree_lock_) {
    fs::SharedLock free_nid_lock(free_nid_tree_lock_);
    return free_nid_tree_.size();
  }

 private:
  void NodeInfoFromRawNat(NodeInfo &ni, RawNatEntry &raw_ne);
  // If the node page at |start| doesn't hit in the cache, do readahead node pages
  // from |start| in |parent|.
  zx::result<LockedPage> GetNextNodePage(LockedPage &parent, size_t start);
  friend class MapTester;
  pgoff_t CurrentNatAddr(nid_t start);
  bool IsUpdatedNatPage(nid_t start);
  pgoff_t NextNatAddr(pgoff_t block_addr);
  void SetToNextNat(nid_t start_nid);

  zx::result<LockedPage> GetCurrentNatPage(nid_t nid);
  zx::result<LockedPage> GetNextNatPage(nid_t nid);
  void RaNatPages(nid_t nid);

  void SetNatCacheDirty(NatEntry &ne) __TA_REQUIRES(nat_tree_lock_);
  void ClearNatCacheDirty(NatEntry &ne) __TA_REQUIRES(nat_tree_lock_);
  NatEntry *LookupNatCache(nid_t n) __TA_REQUIRES_SHARED(nat_tree_lock_);
  uint32_t GangLookupNatCache(uint32_t nr, NatEntry **out) __TA_REQUIRES_SHARED(nat_tree_lock_);
  void DelFromNatCache(NatEntry &entry) __TA_REQUIRES_SHARED(nat_tree_lock_);
  NatEntry *GrabNatEntry(nid_t nid) __TA_REQUIRES_SHARED(nat_tree_lock_);
  void CacheNatEntry(nid_t nid, RawNatEntry &raw_entry);
  int TryToFreeNats(int nr_shrink);

  void BuildFreeNids() __TA_EXCLUDES(free_nid_tree_lock_) __TA_EXCLUDES(build_lock_);
  zx::result<> LookupFreeNidList(nid_t n) __TA_REQUIRES(free_nid_tree_lock_);
  void RemoveFreeNid(nid_t nid) __TA_EXCLUDES(free_nid_tree_lock_);
  void RemoveFreeNidUnsafe(nid_t nid) __TA_REQUIRES(free_nid_tree_lock_);

  int ScanNatPage(Page &nat_page, nid_t start_nid);

  zx_status_t InitNodeManager();

  F2fs *const fs_ = nullptr;
  SuperblockInfo &superblock_info_;
  block_t nat_blkaddr_ = 0;  // starting block address of NAT

  std::shared_mutex nat_tree_lock_;  // protect nat_tree_lock
  uint32_t nat_entries_count_ = 0;   // the number of nat cache entries

  using NatTreeTraits = fbl::DefaultKeyedObjectTraits<nid_t, NatEntry>;
  using NatTree = fbl::WAVLTree<nid_t, std::unique_ptr<NatEntry>, NatTreeTraits>;
  using NatList = fbl::DoublyLinkedList<NatEntry *>;

  NatTree nat_cache_ __TA_GUARDED(nat_tree_lock_);       // cached nat entries
  NatList clean_nat_list_ __TA_GUARDED(nat_tree_lock_);  // a list for cached clean nats
  NatList dirty_nat_list_ __TA_GUARDED(nat_tree_lock_);  // a list for cached dirty nats

  std::shared_mutex free_nid_tree_lock_;               // protect free nid list
  std::mutex build_lock_;                              // lock for building free nids
  nid_t max_nid_ = 0;                                  // the maximum number of node ids
  nid_t next_scan_nid_ __TA_GUARDED(build_lock_) = 0;  // the next nid to be scanned
  std::set<nid_t> free_nid_tree_ __TA_GUARDED(free_nid_tree_lock_);  // tree for free nids

  RawBitmap nat_bitmap_;          // NAT bitmap pointer
  RawBitmap nat_prev_bitmap_;     // NAT previous checkpoint bitmap pointer
  uint32_t nat_bitmap_size_ = 0;  // NAT bitmap size
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_NODE_H_
