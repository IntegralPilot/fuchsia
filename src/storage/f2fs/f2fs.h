// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_F2FS_H_
#define SRC_STORAGE_F2FS_F2FS_H_

#include <condition_variable>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/layout.h"
#include "src/storage/f2fs/memory_watcher.h"
#include "src/storage/f2fs/mount.h"

namespace f2fs {

struct WritebackOperation;
class SuperblockInfo;
class NodeManager;
class VnodeCache;
class SegmentManager;
class NodePage;
class LockedPage;
class VnodeCache;
class Writer;
class Reader;
class VnodeF2fs;
class BcacheMapper;
class InspectTree;
zx::result<std::unique_ptr<Superblock>> LoadSuperblock(BcacheMapper &bc);

// Used to track orphans and modified dirs
enum class VnodeSet {
  kOrphan = 0,
  kModifiedDir,
  kMax,
};

class F2fs final {
 public:
  // Not copyable or moveable
  F2fs(const F2fs &) = delete;
  F2fs &operator=(const F2fs &) = delete;
  F2fs(F2fs &&) = delete;
  F2fs &operator=(F2fs &&) = delete;

  explicit F2fs(FuchsiaDispatcher dispatcher, std::unique_ptr<f2fs::BcacheMapper> bc,
                const MountOptions &mount_options, PlatformVfs *vfs);

  static zx::result<std::unique_ptr<F2fs>> Create(FuchsiaDispatcher dispatcher,
                                                  std::unique_ptr<f2fs::BcacheMapper> bc,
                                                  const MountOptions &options, PlatformVfs *vfs);

  zx::result<fs::FilesystemInfo> GetFilesystemInfo();
  InspectTree &GetInspectTree() { return *inspect_tree_; }

  zx::result<fbl::RefPtr<VnodeF2fs>> GetVnode(ino_t ino, LockedPage *inode_page = nullptr);
  zx::result<fbl::RefPtr<VnodeF2fs>> CreateNewVnode(umode_t mode,
                                                    std::optional<gid_t> gid = std::nullopt);

  VnodeCache &GetVCache() { return *vnode_cache_; }

  zx::result<std::unique_ptr<f2fs::BcacheMapper>> TakeBc();

  BcacheMapper &GetBc() const {
    ZX_DEBUG_ASSERT(bc_ != nullptr);
    return *bc_;
  }
  SuperblockInfo &GetSuperblockInfo() const {
    ZX_DEBUG_ASSERT(superblock_info_ != nullptr);
    return *superblock_info_;
  }
  SegmentManager &GetSegmentManager() const {
    ZX_DEBUG_ASSERT(segment_manager_ != nullptr);
    return *segment_manager_;
  }
  NodeManager &GetNodeManager() const {
    ZX_DEBUG_ASSERT(node_manager_ != nullptr);
    return *node_manager_;
  }
  Writer &GetWriter() { return *writer_; }
  PlatformVfs *vfs() const { return vfs_; }

  zx_status_t LoadSuper(std::unique_ptr<Superblock> sb);
  void Reset();
  zx_status_t GrabMetaPage(pgoff_t index, LockedPage *out);
  zx_status_t GetMetaPage(pgoff_t index, LockedPage *out);
  bool IsTearDown() const;
  void SetTearDown();

  zx_status_t CheckOrphanSpace();
  zx::result<> PurgeOrphanInode(nid_t ino) __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());
  int PurgeOrphanInodes() __TA_EXCLUDES(f2fs::GetGlobalLock());
  void WriteOrphanInodes(block_t start_blk) __TA_REQUIRES(f2fs::GetGlobalLock());
  zx_status_t GetValidCheckpoint();
  zx_status_t ValidateCheckpoint(block_t cp_addr, uint64_t *version, LockedPage *out);

  void PutSuper();
  void Sync(SyncCallback closure = nullptr) __TA_EXCLUDES(f2fs::GetGlobalLock());
  zx_status_t SyncFs(bool bShutdown = false) __TA_EXCLUDES(f2fs::GetGlobalLock());
  zx_status_t SyncFsUnsafe(bool bShutdown = false) __TA_REQUIRES(f2fs::GetGlobalLock());

  zx_status_t DoCheckpoint(bool is_umount) __TA_REQUIRES(f2fs::GetGlobalLock());
  zx_status_t WriteCheckpoint(bool is_umount) __TA_EXCLUDES(f2fs::GetGlobalLock());
  zx_status_t WriteCheckpointUnsafe(bool is_umount) __TA_REQUIRES(f2fs::GetGlobalLock());

  // recovery.cc
  // For the list of fsync inodes, used only during recovery
  class FsyncInodeEntry : public fbl::DoublyLinkedListable<std::unique_ptr<FsyncInodeEntry>> {
   public:
    explicit FsyncInodeEntry(fbl::RefPtr<VnodeF2fs> vnode_refptr)
        : vnode_(std::move(vnode_refptr)) {}

    FsyncInodeEntry() = delete;
    FsyncInodeEntry(const FsyncInodeEntry &) = delete;
    FsyncInodeEntry &operator=(const FsyncInodeEntry &) = delete;
    FsyncInodeEntry(FsyncInodeEntry &&) = delete;
    FsyncInodeEntry &operator=(FsyncInodeEntry &&) = delete;

    block_t GetLastDnodeBlkaddr() const { return last_dnode_blkaddr_; }
    void SetLastDnodeBlkaddr(block_t blkaddr) { last_dnode_blkaddr_ = blkaddr; }
    VnodeF2fs &GetVnode() const { return *vnode_; }
    void SetSize(size_t size) { size_ = size; }
    size_t GetSize() const { return size_; }

   private:
    fbl::RefPtr<VnodeF2fs> vnode_ = nullptr;  // vfs inode pointer
    block_t last_dnode_blkaddr_ = 0;          // block address locating the last dnode
    size_t size_ = 0;                         // the content size retrieved from fsyncd dnodes
  };
  using FsyncInodeList = fbl::DoublyLinkedList<std::unique_ptr<FsyncInodeEntry>>;

  FsyncInodeEntry *GetFsyncInode(FsyncInodeList &inode_list, nid_t ino);
  zx_status_t RecoverDentry(NodePage &ipage, VnodeF2fs &vnode)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());
  zx::result<FsyncInodeList> FindFsyncDnodes() __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());
  zx::result<> CheckIndexInPrevNodes(block_t blkaddr) __TA_REQUIRES(f2fs::GetGlobalLock());
  void DoRecoverData(VnodeF2fs &vnode, NodePage &page) __TA_REQUIRES(f2fs::GetGlobalLock());
  void RecoverData(FsyncInodeList &inode_list) __TA_REQUIRES(f2fs::GetGlobalLock());
  void RecoverFsyncData() __TA_EXCLUDES(f2fs::GetGlobalLock());

  VnodeF2fs &GetNodeVnode() { return *node_vnode_; }
  VnodeF2fs &GetMetaVnode() { return *meta_vnode_; }
  zx::result<fbl::RefPtr<VnodeF2fs>> GetRootVnode() {
    if (root_vnode_) {
      return zx::ok(root_vnode_);
    }
    return zx::error(ZX_ERR_BAD_STATE);
  }

  zx::result<> MakeReadOperations(std::vector<LockedPage> &pages, std::vector<block_t> &addrs,
                                  PageType type, bool is_sync = true);
  zx::result<> MakeReadOperation(LockedPage &page, block_t blk_addr, PageType type,
                                 bool is_sync = true);
  zx::result<> MakeReadOperations(zx::vmo &vmo, std::vector<block_t> &addrs, PageType type,
                                  bool is_sync = true);
  zx_status_t MakeTrimOperation(block_t blk_addr, block_t nblocks) const;
  void ScheduleWritebackAndReclaimPages();

  zx::result<uint32_t> StartGc(uint32_t needed = 0) __TA_REQUIRES(f2fs::GetGlobalLock());
  // It checks if there are enough sections to write |needed| data blocks and a checkpoint pack.
  // Since it considers space for dirty dir and node pages, |needed| should indicate only data
  // blocks. If there is not enough space, it does GC or checkpoint writes to secure free or
  // dirty sections. In addition, it waits for memory reclaim under high memory pressure.
  void BalanceFs(uint32_t needed = 0) __TA_EXCLUDES(f2fs::GetGlobalLock(), writeback_mutex_);
  void AllocateFreeSections(uint32_t needed = 0) __TA_REQUIRES(f2fs::GetGlobalLock());
  bool GetMemoryStatus(MemoryStatus action);
  void WaitForAvailableMemory() __TA_EXCLUDES(writeback_mutex_);

  bool IsOnRecovery() const { return on_recovery_; }
  void SetOnRecovery() { on_recovery_ = true; }
  void ClearOnRecovery() { on_recovery_ = false; }

  void AddToVnodeSet(VnodeSet type, nid_t ino) __TA_EXCLUDES(vnode_set_mutex_);
  void RemoveFromVnodeSet(VnodeSet type, nid_t ino) __TA_EXCLUDES(vnode_set_mutex_);
  bool FindVnodeSet(VnodeSet type, nid_t ino) __TA_EXCLUDES(vnode_set_mutex_);
  size_t GetVnodeSetSize(VnodeSet type) __TA_EXCLUDES(vnode_set_mutex_);
  void ForAllVnodeSet(VnodeSet type, fit::function<void(nid_t)> callback)
      __TA_EXCLUDES(vnode_set_mutex_);
  void ClearVnodeSet() __TA_EXCLUDES(vnode_set_mutex_);

  // for tests
  size_t GetGcRuns() { return num_gc_runs_; }
  bool IsValid() const;
  void SetVfsForTests(std::unique_ptr<PlatformVfs> vfs) { vfs_for_tests_ = std::move(vfs); }
  zx::result<std::unique_ptr<PlatformVfs>> TakeVfsForTests() {
    if (vfs_for_tests_) {
      return zx::ok(std::move(vfs_for_tests_));
    }
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  void SetMemoryPressure(MemoryPressure level) { current_memory_pressure_level_ = level; }

 private:
  void StartMemoryPressureWatcher();

  void FlushDirsAndNodes() __TA_REQUIRES(f2fs::GetGlobalLock());
  pgoff_t FlushDirtyMetaPages(bool is_commit) __TA_REQUIRES(f2fs::GetGlobalLock());
  pgoff_t FlushDirtyDataPages(WritebackOperation &operation)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());
  zx::result<pgoff_t> FlushDirtyNodePages(WritebackOperation &operation)
      __TA_REQUIRES(f2fs::GetGlobalLock());

  std::atomic_flag teardown_flag_ = ATOMIC_FLAG_INIT;

  // It is used for waiters of |memory_cvar_|.
  // It should be acquired before f2fs::GetGlobalLock() to avoid deadlock.
  std::shared_mutex writeback_mutex_;
  std::condition_variable_any memory_cvar_;

  FuchsiaDispatcher dispatcher_;
  PlatformVfs *const vfs_ = nullptr;
  std::unique_ptr<f2fs::BcacheMapper> bc_;

  MountOptions mount_options_;

  std::unique_ptr<SuperblockInfo> superblock_info_;
  std::unique_ptr<SegmentManager> segment_manager_;
  std::unique_ptr<NodeManager> node_manager_;

  std::unique_ptr<Reader> reader_;
  std::unique_ptr<Writer> writer_;

  std::unique_ptr<VnodeF2fs> meta_vnode_;
  std::unique_ptr<VnodeF2fs> node_vnode_;
  fbl::RefPtr<VnodeF2fs> root_vnode_;

  std::unique_ptr<VnodeCache> vnode_cache_;

  bool on_recovery_ = false;  // recovery is doing or not
  // for inode number management
  std::shared_mutex vnode_set_mutex_;
  std::map<ino_t, uint32_t> vnode_set_ __TA_GUARDED(vnode_set_mutex_);
  size_t vnode_set_size_[static_cast<size_t>(VnodeSet::kMax)] __TA_GUARDED(vnode_set_mutex_) = {
      0,
  };

  zx::event fs_id_;
  std::unique_ptr<InspectTree> inspect_tree_;
  std::atomic<MemoryPressure> current_memory_pressure_level_ = MemoryPressure::kUnknown;
  std::unique_ptr<MemoryPressureWatcher> memory_pressure_watcher_;

  // for unittest
  std::unique_ptr<PlatformVfs> vfs_for_tests_;
  size_t num_gc_runs_ = 0;  // # of free sections that gc acquires
};

inline zx::result<std::unique_ptr<f2fs::BcacheMapper>> F2fs::TakeBc() {
  if (!bc_) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  return zx::ok(std::move(bc_));
}

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_F2FS_H_
