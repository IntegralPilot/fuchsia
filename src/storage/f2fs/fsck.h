// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_FSCK_H_
#define SRC_STORAGE_F2FS_FSCK_H_

#include <map>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/layout.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/segment.h"

namespace f2fs {

struct FsckOptions {
  bool repair = false;
  bool verbose = false;
};

struct OrphanInfo {
  uint32_t nr_inodes = 0;
  uint32_t *ino_list = nullptr;
};

struct InodeLinkInfo {
  uint32_t links = 0;
  uint32_t actual_links = 0;
};

struct FsckInfo {
  OrphanInfo orphani;
  struct FsckResult {
    uint64_t valid_block_count = 0;
    uint32_t valid_nat_entry_count = 0;
    uint32_t valid_node_count = 0;
    uint32_t valid_inode_count = 0;
    uint32_t multi_hard_link_files = 0;
  } result;

  std::map<nid_t, InodeLinkInfo> inode_link_map;
  RawBitmap main_area_bitmap;
  RawBitmap nat_area_bitmap;
  std::set<nid_t> data_exist_flag_set;

  uint64_t main_area_bitmap_size = 0;
  uint32_t nat_area_bitmap_size = 0;
  uint64_t nr_main_blocks = 0;
  uint32_t nr_nat_entries = 0;
  uint32_t dentry_depth = 0;
};

enum class NodeType {
  kTypeInode = 37,
  kTypeDirectNode = 43,
  kTypeIndirectNode = 53,
  kTypeDoubleIndirectNode = 67
};

enum class SegType {
  kSegTypeData = 0,
  kSegTypeCurData,
  kSegTypeNode,
  kSegTypeCurNode,
  kSegTypeMax,
};

class ChildInodeInfo : public fbl::DoublyLinkedListable<std::unique_ptr<ChildInodeInfo>> {
 public:
  ChildInodeInfo() = delete;
  ChildInodeInfo(const ChildInodeInfo &) = delete;
  ChildInodeInfo &operator=(const ChildInodeInfo &) = delete;
  ChildInodeInfo(ChildInodeInfo &&) = delete;
  ChildInodeInfo &operator=(ChildInodeInfo &&) = delete;
  ChildInodeInfo(nid_t id, FileType ftype) : id_(id), ftype_(ftype) {}
  nid_t Id() const { return id_; }
  FileType Ftype() const { return ftype_; }

 private:
  const nid_t id_;
  const FileType ftype_ = FileType::kFtUnknown;
};

using ChildList = fbl::DoublyLinkedList<std::unique_ptr<ChildInodeInfo>>;

#if 0  // porting needed
struct DumpOption {
  nid_t nid;
  int start_sit;
  int end_sit;
  int start_ssa;
  int end_ssa;
  uint32_t blk_addr;
};
#endif

struct TraverseResult {
  uint64_t block_count;  // number of blocks occupied by the inode subtree structure.
  uint32_t link_count;   // number of child directories (valid only for directories).
};

class FsckWorker {
 public:
  // Not copyable or movable
  FsckWorker(const FsckWorker &) = delete;
  FsckWorker &operator=(const FsckWorker &) = delete;
  FsckWorker(FsckWorker &&) = delete;
  FsckWorker &operator=(FsckWorker &&) = delete;
  FsckWorker(std::unique_ptr<BcacheMapper> bc, const FsckOptions &options)
      : fsck_options_(options) {
    bc_ = std::move(bc);
  }
  ~FsckWorker() { DoUmount(); }

  zx_status_t ReadBlock(void *buffer, block_t bno);
  zx_status_t WriteBlock(void *buffer, block_t bno);

  // This is the main logic of fsck.
  // It reads and validates a node block, updates the context and traverse along its child blocks.
  zx::result<TraverseResult> CheckNodeBlock(const Inode *inode, nid_t nid, FileType ftype,
                                            NodeType ntype);

  // Even in a successful return, the returned pair can be |{*nullptr*, node_info}| if
  // |node_info.blkaddr| is |kNewAddr|.
  zx::result<NodeInfo> ReadNodeBlock(nid_t nid, BlockBuffer<Node> &block);
  zx_status_t ValidateNodeBlock(const Node &node_block, NodeInfo node_info, FileType ftype,
                                NodeType ntype);
  // This function checks the validity of a node block with respect to the traverse context and
  // updates the context. In a successful return, this function returns a bool value to indicate
  // whether the caller should traverse deeper.
  zx::result<bool> UpdateContext(const Node &node_block, NodeInfo node_info, FileType ftype,
                                 NodeType ntype);

  // Below traverse functions describe how to iterate over for each data structures.
  zx_status_t TraverseInode(nid_t ino, FileType ftype);
  zx::result<TraverseResult> TraverseInodeBlock(const Node &node_block, NodeInfo node_info,
                                                FileType ftype);
  zx::result<TraverseResult> TraverseDnodeBlock(const Inode *inode, const Node &node_block,
                                                NodeInfo node_info, FileType ftype);
  zx::result<TraverseResult> TraverseIndirectNodeBlock(const Inode *inode, const Node &node_block,
                                                       FileType ftype);
  zx::result<TraverseResult> TraverseDoubleIndirectNodeBlock(const Inode *inode,
                                                             const Node &node_block,
                                                             FileType ftype);

  zx_status_t CheckDataBlock(uint32_t block_address, uint32_t &child_count, uint32_t &child_files,
                             int last_block, FileType ftype, uint32_t parent_nid,
                             uint16_t index_in_node, uint8_t ver);
  zx_status_t CheckDentries(uint32_t &child_count, uint32_t &child_files, int last_block,
                            const uint8_t *dentry_bitmap, const DirEntry *dentries,
                            const uint8_t (*filename)[kNameLen], size_t max_entries);
  zx_status_t CheckDentryBlock(uint32_t block_address, uint32_t &child_count, uint32_t &child_files,
                               int last_block);

  void PrintRawSuperblockInfo();
  void PrintCheckpointInfo();
  void PrintInodeInfo(const Inode &inode);
  void PrintNodeInfo(const Node &node_block);

  // Fsck checks f2fs consistency as below.
  // 1. It loads a valid superblock, and it obtains valid node/inode/block count information.
  zx_status_t DoMount();
  // 2. It builds three bitmap:
  //  a) main_area_bitmap indicates valid blocks that DoFsck() will identify.
  //  b) nat_area_bitmap indicates used NIDs retrieved from NAT.
  //     Once DoFsck() identifies a valid NID, it clears the bit.
  //  c) sit_area_bitmap indicates valid blocks retrieved from SIT.
  // DoFsck() references it for checking the block validity.
  zx_status_t Init();
  // 3. It checks orphan nodes, and it updates nat_area_bitmap.
  zx_status_t CheckOrphanNodes();
  // 4. It traverses blocks from the root inode to leaf inodes to check the validity of
  //   the data/node blocks based on SSA and SIT and to update nat_area_bitmap and main_area_bitmap.
  //   In case of dir block, it checks the validity of child dentries and regarding inodes.
  //   It tracks various count information as well.
  zx_status_t DoFsck();
  // 5. It determines the consistency:
  //   a) main_area_bitmap must be the same as sit_area_bitmap
  //   b) all bits in nat_area_bitmap must be clear. That is, no dangling NIDs.
  //   c) The count information that DoFsck() retrieves must be the same as that in 1.
  //   d) no unreachable links
  zx_status_t Verify();

  // If the repair option is set and Verify() fails,
  // fsck will call Repair() to repair specific filesystem flaws and bring consistency.
  zx_status_t Repair();
  // RepairNat() nullifies unreachable NAT entries, including those in the journal.
  zx_status_t RepairNat();
  // RepairSit() nullifies unreachable bits in SIT entries, including those in the journal.
  zx_status_t RepairSit();
  // RepairCheckpoint() corrects members in the checkpoint, including
  // |valid_block_count|, |valid_node_count|, |valid_inode_count|, |cur_node_blkoff|,
  // |cur_data_blkoff|.
  zx_status_t RepairCheckpoint();
  // RepairInodeLinks() iterates over inode link map and corrects link count for each inode.
  zx_status_t RepairInodeLinks();
  // RepairDataExistFlag() sets kDataExist for each inode that has inline data with the flag unset.
  zx_status_t RepairDataExistFlag();
  void DoUmount();
  zx_status_t Run();

  zx_status_t GetValidSuperblock();
  zx::result<std::pair<std::unique_ptr<BlockBuffer<Checkpoint>>, uint64_t>> ValidateCheckpoint(
      block_t cp_addr);
  zx_status_t GetValidCheckpoint();
  zx_status_t InitNodeManager();
  zx_status_t BuildNodeManager();
  zx_status_t BuildSitInfo();
  zx_status_t BuildCurseg();
  zx_status_t BuildSegmentManager();
  void BuildNatAreaBitmap();
  void BuildSitAreaBitmap();
  void BuildSitEntries();

  zx_status_t ReadCompactedSummaries();
  zx_status_t ReadNormalSummaries(CursegType type);

  // Given a node segment index |segno|,
  // this function reads each block's footer in the segment and
  // restores nid part of entries in |summary_block|.
  zx_status_t RestoreNodeSummary(uint32_t segno, SummaryBlock &summary_block);
  SegType GetSumBlockInfo(uint32_t segno, BlockBuffer<SummaryBlock> &summary_block);
  std::pair<SegType, Summary> GetSummaryEntry(uint32_t block_address);
  void ResetCurseg(CursegType type, int modified);
  zx_status_t RestoreCursegSummaries();
  std::unique_ptr<BlockBuffer<SitBlock>> GetCurrentSitPage(uint32_t segno);
  void SegmentInfoFromRawSit(SegmentEntry &segment_entry, const SitEntry &raw_sit);
  void CheckBlockCount(uint32_t segno, const SitEntry &raw_sit);
  zx::result<RawNatEntry> LookupNatInJournal(nid_t nid);
  zx::result<RawNatEntry> GetNatEntry(nid_t nid);
  SegmentEntry &GetSegmentEntry(uint32_t segno);
  uint32_t GetSegmentNumber(uint32_t block_address);
  zx::result<NodeInfo> GetNodeInfo(nid_t nid);
  void AddIntoInodeLinkMap(nid_t nid, uint32_t link_count);
  zx_status_t FindAndIncreaseInodeLinkMap(nid_t nid);

  zx_status_t VerifyCursegOffset(CursegType segtype);

  inline bool IsValidSsaNodeBlock(nid_t nid, uint32_t block_address);
  inline bool IsValidSsaDataBlock(uint32_t block_address, uint32_t parent_nid,
                                  uint16_t index_in_node, uint8_t version);
  bool IsValidNid(nid_t nid);
  bool IsValidBlockAddress(uint32_t addr);
  block_t StartSummaryBlock() {
    return superblock_info_->StartCpAddr() +
           LeToCpu(superblock_info_->GetCheckpoint().cp_pack_start_sum);
  }
  block_t SummaryBlockAddress(int base, int type) {
    return superblock_info_->StartCpAddr() +
           LeToCpu(superblock_info_->GetCheckpoint().cp_pack_total_block_count) - (base + 1) + type;
  }
  static void NodeInfoFromRawNat(NodeInfo &ni, RawNatEntry &raw_nat) {
    ni.ino = LeToCpu(raw_nat.ino);
    ni.blk_addr = LeToCpu(raw_nat.block_addr);
    ni.version = raw_nat.version;
  }

  zx::result<bool> CheckXattrBlock(uint32_t ino, uint32_t x_nid);

#if 0  // porting needed
  void sit_dump(SuperblockInfo *sbi, int start_sit, int end_sit);
  void ssa_dump(SuperblockInfo *sbi, int start_ssa, int end_ssa);
  int dump_node(SuperblockInfo *sbi, nid_t nid);
  int dump_inode_from_blkaddr(SuperblockInfo *sbi, uint32_t blk_addr);
#endif

  std::unique_ptr<BcacheMapper> Destroy() {
    DoUmount();
    return std::move(bc_);
  }

  // For testing
  SegmentManager &GetSegmentManager() const {
    ZX_DEBUG_ASSERT(segment_manager_ != nullptr);
    return *segment_manager_;
  }

  // For testing
  NodeManager &GetNodeManager() const {
    ZX_DEBUG_ASSERT(node_manager_ != nullptr);
    return *node_manager_;
  }

  // For testing
  SuperblockInfo &GetSuperblockInfo() { return *superblock_info_; }

 private:
  // Saves the traverse context. It should be re-initialized every traverse.
  FsckInfo fsck_;
  const FsckOptions fsck_options_;
  std::unique_ptr<SuperblockInfo> superblock_info_;
  std::unique_ptr<NodeManager> node_manager_;
  std::unique_ptr<SegmentManager> segment_manager_;
  std::unique_ptr<BcacheMapper> bc_;

  bool mounted_ = false;

  RawBitmap sit_area_bitmap_;
  uint32_t sit_area_bitmap_size_ = 0;
  ChildList inode_list_;
};

zx_status_t Fsck(std::unique_ptr<BcacheMapper> bc, const FsckOptions &options,
                 std::unique_ptr<BcacheMapper> *out = nullptr);

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_FSCK_H_
