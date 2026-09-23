// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_TEST_UNIT_UNIT_LIB_H_
#define SRC_STORAGE_F2FS_TEST_UNIT_UNIT_LIB_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include <unordered_set>

#include <gtest/gtest.h>

#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/component_runner.h"
#include "src/storage/f2fs/dir.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/file.h"
#include "src/storage/f2fs/file_cache.h"
#include "src/storage/f2fs/fsck.h"
#include "src/storage/f2fs/inspect.h"
#include "src/storage/f2fs/mkfs.h"
#include "src/storage/f2fs/mount.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/reader.h"
#include "src/storage/f2fs/storage_buffer.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vnode_cache.h"
#include "src/storage/f2fs/writeback.h"

namespace f2fs {
constexpr uint64_t kSectorCount50MiB = 102400;
constexpr uint64_t kSectorCount100MiB = 204800;
constexpr uint64_t kDefaultSectorCount = kSectorCount50MiB;
struct TestOptions {
  uint64_t block_count = kDefaultSectorCount;
  uint64_t block_size = kDefaultSectorSize;
  MkfsOptions mkfs_options{};
  std::vector<std::pair<MountOption, size_t>> mount_options;
  bool run_fsck = true;
  // The file given by |image_file| is a zstd compressed image of f2fs, located at "pkg/data"
  std::optional<std::string> image_file;
};

class F2fsFakeDevTestFixture : public testing::Test {
 public:
  F2fsFakeDevTestFixture(const TestOptions &options = TestOptions());
  ~F2fsFakeDevTestFixture() = default;

  void SetUp() override;
  void TearDown() override;
  void Remount();

  void DisableFsck() { run_fsck_ = false; }

 protected:
  uint64_t block_count_;
  uint64_t block_size_;
  MkfsOptions mkfs_options_{};
  MountOptions mount_options_{};
  bool run_fsck_;
  std::unique_ptr<f2fs::BcacheMapper> bc_;
  std::unique_ptr<F2fs> fs_;
  fbl::RefPtr<Dir> root_dir_;
  async::Loop loop_ = async::Loop(&kAsyncLoopConfigAttachToCurrentThread);
  std::optional<std::string> image_file_;

 private:
  void LoadImage();
};

class SingleFileTest : public F2fsFakeDevTestFixture {
 public:
  SingleFileTest(umode_t mode = S_IFREG, const TestOptions &options = TestOptions())
      : F2fsFakeDevTestFixture(options), mode_(mode) {}

  void SetUp() override {
    F2fsFakeDevTestFixture::SetUp();
    zx::result file = root_dir_->CreateWithMode("FileCacheTest", mode_);
    ASSERT_TRUE(file.is_ok()) << file.status_string();
    test_file_ = std::move(*file);
  }

  void TearDown() override {
    CloseVnode();
    F2fsFakeDevTestFixture::TearDown();
  }

  LockedPage GetLockedPage(pgoff_t index) {
    LockedPage page;
    ZX_ASSERT(vnode().GrabLockedPage(index, &page) == ZX_OK);
    return page;
  }

  fbl::RefPtr<Page> GetPage(pgoff_t index) {
    zx::result pages_or = vnode().GrabPages(index, index + 1);
    ZX_ASSERT(pages_or.is_ok());
    return pages_or->front();
  }

  void CloseVnode() {
    if (test_file_) {
      test_file_->Close();
      test_file_.reset();
    }
  }

 protected:
  template <typename T = VnodeF2fs>
  T &vnode() {
    ZX_ASSERT(test_file_);
    return *fbl::RefPtr<T>::Downcast(test_file_);
  }

 private:
  fbl::RefPtr<fs::Vnode> test_file_;
  umode_t mode_ = S_IFREG;
};

class FileTester {
 public:
  static void MkfsOnFakeDev(std::unique_ptr<BcacheMapper> *bc,
                            uint64_t block_count = kDefaultSectorCount,
                            uint32_t block_size = kDefaultSectorSize, bool btrim = true);
  static void MkfsOnFakeDevWithOptions(std::unique_ptr<BcacheMapper> *bc,
                                       const MkfsOptions &options,
                                       uint64_t block_count = kDefaultSectorCount,
                                       uint32_t block_size = kDefaultSectorSize, bool btrim = true);
  static void MountWithOptions(async_dispatcher_t *dispatcher, const MountOptions &options,
                               std::unique_ptr<BcacheMapper> *bc, std::unique_ptr<F2fs> *fs);
  static void Unmount(std::unique_ptr<F2fs> fs, std::unique_ptr<BcacheMapper> *bc);
  static void SuddenPowerOff(std::unique_ptr<F2fs> fs, std::unique_ptr<BcacheMapper> *bc);

  static void CreateRoot(F2fs *fs, fbl::RefPtr<VnodeF2fs> *out);
  static void Lookup(VnodeF2fs *parent, std::string_view name, fbl::RefPtr<fs::Vnode> *out);

  static void CreateChild(Dir *vn, umode_t mode, std::string_view name);
  static void DeleteChild(Dir *vn, std::string_view name, bool is_dir = true);
  static void RenameChild(fbl::RefPtr<Dir> &old_vnode, fbl::RefPtr<Dir> &new_vnode,
                          std::string_view oldname, std::string_view newname);
  static void CreateChildren(F2fs *fs, std::vector<fbl::RefPtr<VnodeF2fs>> &vnodes,
                             std::vector<uint32_t> &inos, fbl::RefPtr<Dir> &parent,
                             std::string name, size_t inode_cnt);
  static void DeleteChildren(std::vector<fbl::RefPtr<VnodeF2fs>> &vnodes, fbl::RefPtr<Dir> &parent,
                             size_t inode_cnt);

  static void VnodeWithoutParent(F2fs *fs, umode_t mode, fbl::RefPtr<VnodeF2fs> &vnode);

  static void CheckInlineDir(VnodeF2fs *vn);
  static void CheckNonInlineDir(VnodeF2fs *vn);

  static void CheckInlineFile(VnodeF2fs *vn);
  static void CheckNonInlineFile(VnodeF2fs *vn);

  static void CheckDataExistFlagSet(VnodeF2fs *vn);
  static void CheckDataExistFlagUnset(VnodeF2fs *vn);

  static void CheckInlineXattr(VnodeF2fs *vn);

  static void CheckChildrenFromReaddir(Dir *dir, std::unordered_set<std::string> childs);
  static void CheckChildrenInBlock(Dir *vn, uint64_t bidx, std::unordered_set<std::string> childs);

  static std::string GetRandomName(unsigned int len);

  static void AppendToInline(File *file, const void *data, size_t len);
  static void AppendToFile(File *file, const void *data, size_t len);
  static void ReadFromFile(File *file, void *data, size_t len, size_t off);

  static zx_status_t Read(File *file, void *data, size_t len, size_t off, size_t *out_actual);
  static zx_status_t Write(File *file, const void *data, size_t len, size_t offset,
                           size_t *out_actual);
  static zx_status_t Append(File *file, const void *data, size_t len, size_t *out_end,
                            size_t *out_actual);
};

class MapTester {
 public:
  static void CheckNodeLevel(F2fs *fs, VnodeF2fs *vn, uint32_t level);
  static void CheckNidsFree(F2fs *fs, std::unordered_set<nid_t> &nids);
  static void CheckNidsInuse(F2fs *fs, std::unordered_set<nid_t> &nids);
  static void CheckBlkaddrsFree(F2fs *fs, std::unordered_set<block_t> &blkaddrs);
  static void CheckBlkaddrsInuse(F2fs *fs, std::unordered_set<block_t> &blkaddrs);
  static void CheckDnodePage(NodePage &page, nid_t exp_nid);
  static void DoWriteNat(F2fs *fs, nid_t nid, block_t blkaddr, uint8_t version);
  static void DoWriteSit(F2fs *fs, CursegType type, uint32_t exp_segno, block_t *new_blkaddr);
  static void RemoveTruncatedNode(NodeManager &node_manager, std::vector<nid_t> &nids)
      __TA_EXCLUDES(node_manager.nat_tree_lock_);
  static bool IsCachedNat(NodeManager &node_manager, nid_t n)
      __TA_EXCLUDES(node_manager.nat_tree_lock_);
  static void RemoveAllNatEntries(NodeManager &manager) __TA_EXCLUDES(manager.nat_tree_lock_);
  static nid_t ScanFreeNidList(NodeManager &manager) __TA_EXCLUDES(manager.free_nid_tree_lock_);
  static void GetCachedNatEntryBlockAddress(NodeManager &manager, nid_t nid, block_t &out)
      __TA_EXCLUDES(manager.free_nid_tree_lock_);
  static void SetCachedNatEntryBlockAddress(NodeManager &manager, nid_t nid, block_t address)
      __TA_EXCLUDES(manager.free_nid_tree_lock_);
  static void SetCachedNatEntryCheckpointed(NodeManager &manager, nid_t nid)
      __TA_EXCLUDES(manager.free_nid_tree_lock_);
  static nid_t GetNextFreeNidInList(NodeManager &manager)
      __TA_EXCLUDES(manager.free_nid_tree_lock_) {
    std::lock_guard nat_lock(manager.free_nid_tree_lock_);
    return manager.free_nid_tree_.empty() ? 0 : *manager.free_nid_tree_.begin();
  }
  static void GetNatCacheEntryCount(NodeManager &manager, size_t &num_tree, size_t &num_clean,
                                    size_t &num_dirty) __TA_EXCLUDES(manager.nat_tree_lock_) {
    std::lock_guard nat_lock(manager.nat_tree_lock_);
    num_tree = manager.nat_cache_.size();
    num_clean = manager.clean_nat_list_.size_slow();
    num_dirty = manager.dirty_nat_list_.size_slow();
  }
  static void SetNatCount(NodeManager &manager, uint32_t count) {
    manager.nat_entries_count_ = count;
  }
  static pgoff_t GetCurrentNatAddr(NodeManager &manager, nid_t start) {
    return manager.CurrentNatAddr(start);
  }
};

class MkfsTester {
 public:
  static GlobalParameters &GetGlobalParameters(MkfsWorker &mkfs) { return mkfs.params_; }

  static zx_status_t InitAndGetDeviceInfo(MkfsWorker &mkfs);
  static zx::result<std::unique_ptr<BcacheMapper>> FormatDevice(MkfsWorker &mkfs);
};

class GcTester {
 public:
  static zx_status_t DoGarbageCollect(SegmentManager &manager, uint32_t segno, GcType gc_type)
      __TA_EXCLUDES(f2fs::GetGlobalLock());
  static zx_status_t GcDataSegment(SegmentManager &manager, const SummaryBlock &sum_blk,
                                   unsigned int segno, GcType gc_type)
      __TA_EXCLUDES(f2fs::GetGlobalLock());
};

class DeviceTester {
 public:
  using Hook = std::function<zx_status_t(const BlockFifoRequest &request, const zx::vmo *vmo)>;
  static void SetHook(F2fs *fs, Hook hook);
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_TEST_UNIT_UNIT_LIB_H_
