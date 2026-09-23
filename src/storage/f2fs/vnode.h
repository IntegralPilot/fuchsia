// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_VNODE_H_
#define SRC_STORAGE_F2FS_VNODE_H_

#include <span>

#include "src/storage/f2fs/bitmap.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/dir_entry_cache.h"
#include "src/storage/f2fs/extent_cache.h"
#include "src/storage/f2fs/file_cache.h"
#include "src/storage/f2fs/timestamp.h"
#include "src/storage/f2fs/xattr.h"
#include "src/storage/lib/vfs/cpp/paged_vnode.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"
#include "src/storage/lib/vfs/cpp/vnode.h"
#include "src/storage/lib/vfs/cpp/watcher.h"

namespace f2fs {
constexpr uint32_t kNullIno = std::numeric_limits<uint32_t>::max();
// The maximum number of blocks that an append can make dirty.
// (inode + double indirect + indirect + dnode + data)
constexpr uint32_t kMaxNeededBlocksForUpdate = 5;

struct NodePath;
class F2fs;
class NodePage;
class SuperblockInfo;
class VmoManager;
class DirEntryCache;

// i_advise uses Fadvise:xxx bit. We can add additional hints later.
enum class FAdvise {
  kCold = 1,
};

// InodeInfo->flags keeping only in memory
enum class InodeInfoFlag {
  kInit = 0,      // not used
  kActive,        // indicate open_count > 0
  kNewInode,      // indicate newly allocated vnode
  kNeedCp,        // need to do checkpoint during fsync
  kIncLink,       // need to increment i_nlink
  kAclMode,       // indicate acl mode
  kNoAlloc,       // should not allocate any blocks
  kUpdateDir,     // should update inode block for consistency
  kInlineXattr,   // used for inline xattr
  kInlineData,    // used for inline data
  kInlineDentry,  // used for inline dentry
  kDataExist,     // indicate data exists
  kOrphan,        // purged once open_count() drops to zero
  kNoExtent,      // not to use the extent cache
  kSyncInode,     // need to write its inode block during fdatasync
  kFlagSize,
};

inline bool IsValidNameLength(std::string_view name) { return name.length() <= kMaxNameLen; }

class VnodeF2fs : public fs::PagedVnode,
                  public fbl::Recyclable<VnodeF2fs>,
                  public fbl::WAVLTreeContainable<VnodeF2fs*>,
                  public fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>> {
 public:
  explicit VnodeF2fs(F2fs* fs, ino_t ino, umode_t mode, LockedPage node_page = {});
  virtual ~VnodeF2fs();

  uint32_t InlineDataOffset() const {
    return kBlockSize - sizeof(NodeFooter) -
           sizeof(uint32_t) * (kAddrsPerInode + kNidsPerInode - 1) + extra_isize_;
  }
  size_t MaxInlineData() const { return sizeof(uint32_t) * (GetAddrsPerInode() - 1); }
  size_t MaxInlineDentry() const {
    return GetBitSize(MaxInlineData()) / (GetBitSize(kSizeOfDirEntry + kDentrySlotLen) + 1);
  }
  size_t GetAddrsPerInode() const {
    return safemath::checked_cast<size_t>(
        (safemath::CheckSub(kAddrsPerInode, safemath::CheckDiv(extra_isize_, sizeof(uint32_t))) -
         inline_xattr_size_)
            .ValueOrDie());
  }

  void InitializeFromPage(LockedPage& node_page) __TA_REQUIRES(mutex_);
  zx_status_t CreateFileCacheUnsafe(uint64_t nbytes = 0) __TA_REQUIRES(mutex_);
  zx_status_t CreateFileCache(uint64_t nbytes = 0) __TA_EXCLUDES(mutex_);

  ino_t GetKey() const { return ino_; }

  void Sync(SyncCallback closure) override;
  zx_status_t SyncFile(bool datasync = true) __TA_EXCLUDES(f2fs::GetGlobalLock(), mutex_);

  void fbl_recycle() { RecycleNode(); }

  F2fs* fs() const { return fs_; }

  ino_t Ino() const { return ino_; }

  zx::result<fs::VnodeAttributes> GetAttributes() const final __TA_EXCLUDES(mutex_);
  fs::VnodeAttributesQuery SupportedMutableAttributes() const final;
  zx::result<> UpdateAttributes(const fs::VnodeAttributesUpdate& attributes) final
      __TA_EXCLUDES(mutex_);

  fuchsia_io::NodeProtocolKinds GetProtocols() const override;

  // For fs::PagedVnode
  zx_status_t GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) override
      __TA_REQUIRES(mutex_);
  void VmoRead(uint64_t offset, uint64_t length) override __TA_REQUIRES_SHARED(mutex_);
  void VmoDirty(uint64_t offset, uint64_t length) override __TA_REQUIRES_SHARED(mutex_);
  zx::result<size_t> CreateAndPopulateVmo(zx::vmo& vmo, const size_t offset, const size_t length)
      __TA_REQUIRES_SHARED(mutex_);
  void OnNoPagedVmoClones() final __TA_REQUIRES(mutex_);
  void ReleasePagedVmo() __TA_REQUIRES(mutex_);

  virtual zx_status_t InitInodeMetadata() __TA_EXCLUDES(mutex_)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());
  virtual zx_status_t InitInodeMetadataUnsafe() __TA_REQUIRES(mutex_)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());
  zx::result<LockedPage> NewInodePage();
  void UpdateInodePage(LockedPage& inode_page, bool checkpoint = false) __TA_EXCLUDES(mutex_);
  void UpdateInodePageUnsafe(LockedPage& inode_page, bool update_size = false)
      __TA_REQUIRES_SHARED(mutex_);

  // truncate
  zx_status_t DoTruncate(size_t len) __TA_REQUIRES_SHARED(f2fs::GetGlobalLock())
      __TA_REQUIRES(mutex_);
  zx_status_t TruncateBlocks(uint64_t from) __TA_REQUIRES_SHARED(f2fs::GetGlobalLock())
      __TA_REQUIRES(mutex_);
  void TruncateNode(LockedPage& page) __TA_REQUIRES(mutex_);
  block_t TruncateDnodeAddrs(LockedPage& dnode, size_t offset, size_t count) __TA_REQUIRES(mutex_);
  zx::result<size_t> TruncateDnode(nid_t nid) __TA_REQUIRES(mutex_);
  zx::result<size_t> TruncateNodes(nid_t start_nid, size_t nofs, size_t ofs, size_t depth)
      __TA_REQUIRES(mutex_);
  zx_status_t TruncatePartialNodes(const Inode& inode, const size_t (&offset)[4], size_t depth)
      __TA_REQUIRES(mutex_);
  zx_status_t TruncateInodeBlocks(pgoff_t from) __TA_REQUIRES(mutex_);
  zx_status_t InvalidateBlocks(size_t from) __TA_REQUIRES(mutex_);

  zx_status_t TruncateHole(pgoff_t pg_start, pgoff_t pg_end, bool evict = true)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock()) __TA_EXCLUDES(mutex_);
  zx_status_t TruncateHoleUnsafe(pgoff_t pg_start, pgoff_t pg_end, bool evict = true)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock()) __TA_REQUIRES(mutex_);

  // handling orphans
  void SetOrphan() __TA_EXCLUDES(mutex_) __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());
  zx_status_t PurgeInodeBlock() __TA_REQUIRES(mutex_);
  void PurgeDataBlocks() __TA_REQUIRES_SHARED(f2fs::GetGlobalLock()) __TA_REQUIRES(mutex_);
  void PurgeUnsafe() __TA_REQUIRES_SHARED(f2fs::GetGlobalLock()) __TA_REQUIRES(mutex_);
  void Purge() __TA_REQUIRES_SHARED(f2fs::GetGlobalLock()) __TA_EXCLUDES(mutex_);

  // Reserve space for a new block
  zx_status_t GetNewDataPage(pgoff_t index, LockedPage* out)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock()) __TA_REQUIRES(mutex_);
  zx_status_t ReserveNewBlock(LockedPage& node_page, size_t ofs_in_node);

  // Returns the block addresses for |indices| or |index|, reserving space as needed.
  zx::result<std::vector<block_t>> GetAddresses(const std::vector<pgoff_t>& indices)
      __TA_REQUIRES(mutex_);
  zx::result<std::vector<block_t>> GetAddresses(pgoff_t index, size_t count) __TA_REQUIRES(mutex_);
  // Returns the block addresses for |indices| or |index| without reservation.
  zx::result<std::vector<block_t>> FindAddresses(const std::vector<pgoff_t>& indices);
  zx::result<std::vector<block_t>> FindAddresses(pgoff_t index, size_t count);

  // Return a block addr for page. It allocates a block addr as needed.
  virtual block_t GetBlockAddr(LockedPage& page);

  // It reserves space from |offset| to |offset| + |len| before dirtying the corresponding pages.
  zx::result<std::vector<LockedPage>> WriteBegin(const size_t offset, const size_t len)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock()) __TA_REQUIRES(mutex_);
  pgoff_t Writeback(WritebackOperation& operation) __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());

  // The maximum depth is four.
  // Offset[0] indicates inode offset.
  zx::result<NodePath> GetNodePath(pgoff_t block);

  virtual zx::result<PageBitmap> GetBitmap(fbl::RefPtr<Page> dentry_page);

  void Notify(std::string_view name, fuchsia_io::wire::WatchEvent event) final;
  zx_status_t WatchDir(fs::FuchsiaVfs* vfs, fuchsia_io::wire::WatchMask mask, uint32_t options,
                       fidl::ServerEnd<fuchsia_io::DirectoryWatcher> watcher) final;

  bool ExtentCacheAvailable();
  void InitExtentTree();
  void UpdateExtentCache(pgoff_t file_offset, block_t blk_addr, uint32_t len = 1);
  zx::result<block_t> LookupExtentCacheBlock(pgoff_t file_offset);

  void IncrementLink() __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    IncrementLinkUnsafe();
  }
  void ClearLinkCount() __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    ClearLinkCountUnsafe();
  }
  void DecrementLink() __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    DecrementLinkUnsafe();
  }
  uint32_t GetLinkCount() const __TA_EXCLUDES(mutex_) {
    fs::SharedLock lock(mutex_);
    return GetLinkCountUnsafe();
  }

  void SetMode(const umode_t& mode);
  umode_t GetMode() const;
  bool IsDir() const;
  bool IsReg() const;
  bool IsLink() const;
  bool IsChr() const;
  bool IsBlk() const;
  bool IsSock() const;
  bool IsFifo() const;
  bool HasGid() const;
  bool IsMeta() const;
  bool IsNode() const;

  void SetName(std::string_view name) __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    SetTime<Timestamps::ChangeTime>();
    name_ = name;
  }
  bool IsSameName(std::string_view name) __TA_EXCLUDES(mutex_) {
    fs::SharedLock rlock(mutex_);
    return std::string_view(name_).compare(name) == 0;
  }
  const std::string& GetNameView() const __TA_EXCLUDES(mutex_) {
    fs::SharedLock rlock(mutex_);
    return GetNameViewUnsafe();
  }

  void AddBlocks(const block_t nblocks) __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    return AddBlocksUnsafe(nblocks);
  }
  void RemoveBlocks(const block_t nblocks) __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    return RemoveBlocksUnsafe(nblocks);
  }
  block_t GetBlockCount() const __TA_EXCLUDES(mutex_) {
    fs::SharedLock lock(mutex_);
    return GetBlockCountUnsafe();
  }

  // It adjusts both the size and the contents of its VMO.
  // If the vnode uses a paged VMO, it can invoke VmoDirty() as needed, and the kernel then
  // guarantees that the VMO’s contents beyond |len| are zero-initialized.
  virtual void SetSize(const size_t nbytes) __TA_EXCLUDES(mutex_);
  uint64_t GetSize(bool update_checkpointed_size = false) const;

  void SetParentNid(const ino_t pino) { parent_ino_.store(pino, std::memory_order_release); }
  ino_t GetParentNid() const { return parent_ino_.load(std::memory_order_acquire); }

  void IncreaseDirtyPageCount() { dirty_pages_.fetch_add(1); }
  void DecreaseDirtyPageCount() { dirty_pages_.fetch_sub(1); }
  block_t GetDirtyPageCount() const { return dirty_pages_.load(std::memory_order_acquire); }

  void SetUid(const uid_t uid) { uid_ = uid; }
  void SetGid(const gid_t gid) { gid_ = gid; }

  template <typename T>
  void SetTime(const timespec& time) __TA_REQUIRES(mutex_) {
    time_->Update<T>(time);
  }

  template <typename U>
  void SetTime() __TA_REQUIRES(mutex_) {
    time_->Update<U>();
  }

  void ResetTime() __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    timespec cur_time;
    clock_gettime(CLOCK_REALTIME, &cur_time);
    SetTime<Timestamps::ModificationTime>(cur_time);
    SetTime<Timestamps::BirthTime>(cur_time);
  }

  // Coldness identification:
  //  - Mark cold files in InodeInfo
  //  - Mark cold node blocks in their node footer
  //  - Mark cold data pages in page cache
  bool IsColdFile();
  void SetColdFile() __TA_EXCLUDES(mutex_);

  void SetAdvise(const FAdvise bit) { advise_ |= GetMask(1, static_cast<size_t>(bit)); }
  bool IsAdviseSet(const FAdvise bit) {
    return (GetMask(1, static_cast<size_t>(bit)) & advise_) != 0;
  }

  // Set dirty flag and insert |this| to VnodeCache::dirty_list_.
  bool SetDirty();
  bool ClearDirty();
  bool IsDirty();

  void SetInlineXattrSize(const uint16_t num_addrs) { inline_xattr_size_ = num_addrs; }

  // Release-acquire ordering for Set/ClearFlag and TestFlag
  bool SetFlag(const InodeInfoFlag& flag) {
    return flags_[static_cast<uint8_t>(flag)].test_and_set(std::memory_order_release);
  }
  void ClearFlag(const InodeInfoFlag& flag) {
    flags_[static_cast<uint8_t>(flag)].clear(std::memory_order_release);
  }
  bool TestFlag(const InodeInfoFlag& flag) const {
    return flags_[static_cast<uint8_t>(flag)].test(std::memory_order_acquire);
  }

  bool IsActive() const;
  void PutRefPtr() __TA_EXCLUDES(ref_mutex_);
  zx::result<fbl::RefPtr<VnodeF2fs>> GetRefPtr() __TA_EXCLUDES(ref_mutex_);

  zx_status_t FindPage(pgoff_t index, fbl::RefPtr<Page>* out) {
    return file_cache_->FindPage(index, out);
  }

  std::vector<LockedPage> FindLockedPages(pgoff_t start, pgoff_t end) {
    return file_cache_->FindLockedPages(start, end);
  }

  zx_status_t GrabLockedPage(pgoff_t index, LockedPage* out) {
    return file_cache_->GetLockedPage(index, out);
  }

  zx::result<std::vector<fbl::RefPtr<Page>>> GrabPages(pgoff_t start, pgoff_t end) {
    return file_cache_->GetPages(start, end);
  }

  zx::result<std::vector<LockedPage>> GrabLockedPages(pgoff_t start, pgoff_t end) {
    return file_cache_->GetLockedPages(start, end);
  }

  virtual zx::result<LockedPage> FindVictimPage(pgoff_t index) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  size_t GetPageCount() { return file_cache_->GetSize(); }

  std::vector<LockedPage> InvalidatePages(pgoff_t start = 0, pgoff_t end = kPgOffMax,
                                          bool zero = true) {
    return file_cache_->InvalidatePages(start, end, zero);
  }

  DirEntryCache& GetDirEntryCache() { return dir_entry_cache_; }
  VmoManager& GetVmoManager() { return vmo_manager(); }
  const VmoManager& GetVmoManager() const { return vmo_manager(); }
  block_t GetReadBlockSize(block_t start_block, block_t req_size, block_t end_block);
  void CleanupCache();
  uint8_t GetDirLevel() const { return dir_level_; }
  template <typename T>
  const timespec& GetTime() const __TA_REQUIRES_SHARED(mutex_) {
    ZX_DEBUG_ASSERT(time_);
    return time_->Get<T>();
  }

  zx_status_t RecoverInode(NodePage& node_page) __TA_EXCLUDES(mutex_)
      __TA_REQUIRES_SHARED(f2fs::GetGlobalLock());
  virtual zx_status_t RecoverInlineData(NodePage& node_page) { return ZX_ERR_NOT_SUPPORTED; }
  // for testing
  zx_status_t SetExtendedAttribute(XattrIndex index, std::string_view name,
                                   std::span<const uint8_t> value,
                                   XattrOption option) TA_NO_THREAD_SAFETY_ANALYSIS;
  zx::result<size_t> GetExtendedAttribute(XattrIndex index, std::string_view name,
                                          std::span<uint8_t> out) TA_NO_THREAD_SAFETY_ANALYSIS;
  nid_t XattrNid() const { return xattr_nid_; }

  void SetBlockCount(const block_t blocks) TA_NO_THREAD_SAFETY_ANALYSIS { num_blocks_ = blocks; }
  FileCache& GetFileCache() { return *file_cache_; }
  void ResetFileCache() { file_cache_->Reset(); }
  ExtentTree& GetExtentTree() { return *extent_tree_; }
  bool HasPagedVmo() TA_NO_THREAD_SAFETY_ANALYSIS { return paged_vmo().is_valid(); }
  void ClearAdvise(const FAdvise bit) { advise_ &= ~GetMask(1, static_cast<size_t>(bit)); }
  void SetDirLevel(const uint8_t level) { dir_level_ = level; }
  pgoff_t Writeback(bool is_sync = false, bool is_reclaim = false)
      __TA_EXCLUDES(f2fs::GetGlobalLock()) {
    fs::SharedLock lock(f2fs::GetGlobalLock());
    WritebackOperation op = {.bSync = is_sync, .bReclaim = is_reclaim};
    return Writeback(op);
  }

 protected:
  const std::string& GetNameViewUnsafe() const __TA_REQUIRES_SHARED(mutex_) { return name_; }
  void AddBlocksUnsafe(const block_t nblocks) __TA_REQUIRES(mutex_) {
    if (!nblocks) {
      return;
    }
    SetFlag(InodeInfoFlag::kSyncInode);
    num_blocks_ += nblocks;
  }
  void RemoveBlocksUnsafe(const block_t nblocks) __TA_REQUIRES(mutex_) {
    ZX_DEBUG_ASSERT(nblocks > 0);
    ZX_DEBUG_ASSERT(num_blocks_ >= nblocks);
    SetFlag(InodeInfoFlag::kSyncInode);
    num_blocks_ -= nblocks;
  }
  block_t GetBlockCountUnsafe() const __TA_REQUIRES_SHARED(mutex_) { return num_blocks_; }

  void IncrementLinkUnsafe() __TA_REQUIRES(mutex_) {
    ++nlink_;
    SetTime<Timestamps::ChangeTime>();
  }
  void ClearLinkCountUnsafe() __TA_REQUIRES(mutex_) {
    nlink_ = 0;
    SetTime<Timestamps::ChangeTime>();
  }
  void DecrementLinkUnsafe() __TA_REQUIRES(mutex_) {
    --nlink_;
    SetTime<Timestamps::ChangeTime>();
  }
  uint32_t GetLinkCountUnsafe() const __TA_REQUIRES_SHARED(mutex_) { return nlink_; }

  block_t GetBlockAddrOnDataSegment(LockedPage& page);

  zx_status_t CloseNode() override __TA_EXCLUDES(f2fs::GetGlobalLock(), mutex_);
  void RecycleNode() override;
  VmoManager& vmo_manager() const { return *vmo_manager_; }
  void ReportPagerError(const uint32_t op, const uint64_t offset, const uint64_t length,
                        const zx_status_t err) __TA_EXCLUDES(mutex_);
  void ReportPagerErrorUnsafe(const uint32_t op, const uint64_t offset, const uint64_t length,
                              const zx_status_t err) __TA_REQUIRES_SHARED(mutex_);

  bool NeedToSyncDir() const;
  bool NeedToCheckpoint();
  bool NeedToLogInode() const;

  zx::result<size_t> CreatePagedVmo(uint64_t size) __TA_REQUIRES(mutex_);
  zx_status_t ClonePagedVmo(fuchsia_io::wire::VmoFlags flags, size_t size, zx::vmo* out_vmo)
      __TA_REQUIRES(mutex_);
  void SetPagedVmoName() __TA_REQUIRES(mutex_);
  const SuperblockInfo& GetSuperblockInfo() const { return superblock_info_; }
  uint16_t GetExtraSize() const { return extra_isize_; }
  uint64_t GetCurrentDepth() const __TA_REQUIRES_SHARED(mutex_) { return current_depth_; }
  void SetCurrentDepth(uint64_t depth) __TA_REQUIRES(mutex_) { current_depth_ = depth; }

 private:
  DirEntryCache dir_entry_cache_;
  std::unique_ptr<VmoManager> vmo_manager_;
  std::unique_ptr<FileCache> file_cache_;

  SuperblockInfo& superblock_info_;

  uid_t uid_ = 0;
  gid_t gid_ = 0;
  uint32_t generation_ = 0;

  std::string name_ __TA_GUARDED(mutex_);
  block_t num_blocks_ __TA_GUARDED(mutex_) = 0;
  uint32_t nlink_ __TA_GUARDED(mutex_) = 0;
  uint64_t current_depth_ __TA_GUARDED(mutex_) = 1;  // use only in directory structure
  std::unique_ptr<Timestamps> time_ __TA_GUARDED(mutex_);

  std::mutex ref_mutex_;
  std::atomic<block_t> dirty_pages_ = 0;  // # of dirty dentry/data pages
  std::atomic<ino_t> parent_ino_ = kNullIno;
  std::array<std::atomic_flag, static_cast<uint8_t>(InodeInfoFlag::kFlagSize)> flags_ = {
      ATOMIC_FLAG_INIT};
  std::condition_variable_any flag_cvar_;

  uint32_t inode_flags_ = 0;                // keep an inode flags for ioctl
  std::atomic<uint64_t> data_version_ = 0;  // data version from the most recent fsync
  uint16_t extra_isize_ = 0;                // extra inode attribute size in bytes
  uint16_t inline_xattr_size_ = 0;          // inline xattr size
  [[maybe_unused]] umode_t acl_mode_ = 0;   // keep file acl mode temporarily
  uint8_t advise_ = 0;                      // use to give file attribute hints
  uint8_t dir_level_ = 0;                   // use for dentry level for large dir
  // TODO: revisit thread annotation when xattr is available.
  nid_t xattr_nid_ = 0;  // node id that contains xattrs

  std::unique_ptr<ExtentTree> extent_tree_;

  const ino_t ino_ = 0;
  F2fs* const fs_ = nullptr;
  umode_t mode_ = 0;
  fs::WatcherContainer watcher_{};
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_VNODE_H_
