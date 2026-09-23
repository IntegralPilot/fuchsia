// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "unit_lib.h"

#include <fcntl.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <zstd/zstd.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"
#include "src/storage/volume_image/utils/lz4_decompress_reader.h"
#include "src/storage/volume_image/utils/lz4_decompressor.h"

namespace f2fs {

using block_client::FakeBlockDevice;
using Runner = ComponentRunner;

F2fsFakeDevTestFixture::F2fsFakeDevTestFixture(const TestOptions &options)
    : block_count_(options.block_count),
      block_size_(options.block_size),
      run_fsck_(options.run_fsck),
      image_file_(options.image_file) {
  mkfs_options_ = options.mkfs_options;
  for (auto opt : options.mount_options) {
    mount_options_.SetValue(opt.first, opt.second);
  }
}

void F2fsFakeDevTestFixture::SetUp() {
  if (image_file_ != std::nullopt)
    return LoadImage();

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::MkfsOnFakeDevWithOptions(&bc_, mkfs_options_, block_count_);
  FileTester::MountWithOptions(loop_.dispatcher(), mount_options_, &bc_, &fs_);
  FileTester::CreateRoot(fs_.get(), &root);
  root_dir_ = fbl::RefPtr<Dir>::Downcast(std::move(root));
}

void F2fsFakeDevTestFixture::TearDown() {
  ASSERT_EQ(root_dir_->Close(), ZX_OK);
  root_dir_ = nullptr;
  FileTester::Unmount(std::move(fs_), &bc_);

  if (run_fsck_) {
    FsckWorker fsck(std::move(bc_), FsckOptions{.repair = false});
    ASSERT_EQ(fsck.Run(), ZX_OK);
  }
}

void F2fsFakeDevTestFixture::Remount() {
  if (!root_dir_ || !fs_) {
    return;
  }
  ASSERT_EQ(root_dir_->Close(), ZX_OK);
  root_dir_ = nullptr;
  FileTester::Unmount(std::move(fs_), &bc_);
  FileTester::MountWithOptions(loop_.dispatcher(), mount_options_, &bc_, &fs_);
  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs_.get(), &root);
  root_dir_ = fbl::RefPtr<Dir>::Downcast(std::move(root));
}

void F2fsFakeDevTestFixture::LoadImage() {
  auto device =
      std::make_unique<block_client::FakeBlockDevice>(block_client::FakeBlockDevice::Config{
          .block_count = block_count_, .block_size = kDefaultSectorSize, .supports_trim = false});
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());
  bc_ = std::move(*bc_or);

  std::string file = "pkg/data/" + image_file_.value();
  auto fd = fbl::unique_fd(open(file.data(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());

  struct stat s;
  fstat(fd.get(), &s);

  size_t compressed_size = s.st_size;
  auto compressed = std::make_unique<char[]>(compressed_size);
  size_t iter = 0;
  while (iter < compressed_size) {
    size_t cur = read(fd.get(), &(compressed[iter]), compressed_size - iter);
    iter += cur;
  }

  size_t uncompressed_size = block_count_ * block_size_;
  auto uncompressed = std::make_unique<char[]>(uncompressed_size);
  ZSTD_decompress(uncompressed.get(), uncompressed_size, compressed.get(), compressed_size);

  for (uint32_t i = 0; i < uncompressed_size / bc_->BlockSize(); ++i) {
    ASSERT_EQ(bc_->Writeblk(i, uncompressed.get() + i * bc_->BlockSize()), ZX_OK);
  }

  if (run_fsck_) {
    FsckWorker fsck(std::move(bc_), FsckOptions{.repair = false});
    ASSERT_EQ(fsck.Run(), ZX_OK);
    bc_ = fsck.Destroy();
  }

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::MountWithOptions(loop_.dispatcher(), mount_options_, &bc_, &fs_);
  FileTester::CreateRoot(fs_.get(), &root);
  root_dir_ = fbl::RefPtr<Dir>::Downcast(std::move(root));
}

void FileTester::MkfsOnFakeDev(std::unique_ptr<BcacheMapper> *bc, uint64_t block_count,
                               uint32_t block_size, bool btrim) {
  auto device = std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
      .block_count = block_count, .block_size = block_size, .supports_trim = btrim});
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  MkfsOptions options;
  MkfsWorker mkfs(std::move(*bc_or), options);
  auto ret = mkfs.DoMkfs();
  ASSERT_EQ(ret.is_error(), false);
  *bc = std::move(*ret);
}

void FileTester::MkfsOnFakeDevWithOptions(std::unique_ptr<BcacheMapper> *bc,
                                          const MkfsOptions &options, uint64_t block_count,
                                          uint32_t block_size, bool btrim) {
  auto device = std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
      .block_count = block_count, .block_size = block_size, .supports_trim = btrim});
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  MkfsWorker mkfs(std::move(*bc_or), options);
  auto ret = mkfs.DoMkfs();
  ASSERT_EQ(ret.is_error(), false);
  *bc = std::move(*ret);
}

void FileTester::MountWithOptions(async_dispatcher_t *dispatcher, const MountOptions &options,
                                  std::unique_ptr<BcacheMapper> *bc, std::unique_ptr<F2fs> *fs) {
  // Create a vfs object for unit tests.
  auto vfs_or = Runner::CreateRunner(dispatcher);
  ASSERT_TRUE(vfs_or.is_ok());
  auto readonly_or = options.GetValue(MountOption::kReadOnly);
  if (*readonly_or) {
    vfs_or->SetReadonly(true);
  }
  auto fs_or = F2fs::Create(nullptr, std::move(*bc), options, (*vfs_or).get());
  ASSERT_TRUE(fs_or.is_ok());
  (*fs_or)->SetVfsForTests(std::move(*vfs_or));
  *fs = std::move(*fs_or);
}

void FileTester::Unmount(std::unique_ptr<F2fs> fs, std::unique_ptr<BcacheMapper> *bc) {
  fs->Sync();
  fs->PutSuper();
  auto vfs_or = fs->TakeVfsForTests();
  ASSERT_TRUE(vfs_or.is_ok());
  auto bc_or = fs->TakeBc();
  ASSERT_TRUE(bc_or.is_ok());
  *bc = std::move(*bc_or);
  // Trigger teardown before deleting fs.
  (*vfs_or).reset();
}

void FileTester::SuddenPowerOff(std::unique_ptr<F2fs> fs, std::unique_ptr<BcacheMapper> *bc) {
  fs->GetVCache().ForDirtyVnodesIf([&](fbl::RefPtr<VnodeF2fs> &vnode) {
    vnode->ResetFileCache();
    vnode->ClearDirty();
    return ZX_OK;
  });
  fs->GetVCache().Reset();

  // destroy f2fs internal modules
  fs->Reset();

  auto vfs_for_tests = fs->TakeVfsForTests();
  ASSERT_TRUE(vfs_for_tests.is_ok());
  auto bc_or = fs->TakeBc();
  ASSERT_TRUE(bc_or.is_ok());
  *bc = std::move(*bc_or);
  // Trigger teardown before deleting fs.
  (*vfs_for_tests).reset();
}

void FileTester::CreateRoot(F2fs *fs, fbl::RefPtr<VnodeF2fs> *out) {
  auto vnode_or = fs->GetVnode(fs->GetSuperblockInfo().GetRootIno());
  ASSERT_TRUE(vnode_or.is_ok());
  ASSERT_EQ(vnode_or->Open(nullptr), ZX_OK);
  *out = *std::move(vnode_or);
}

void FileTester::Lookup(VnodeF2fs *parent, std::string_view name, fbl::RefPtr<fs::Vnode> *out) {
  fbl::RefPtr<fs::Vnode> vn = nullptr;
  if (zx_status_t status = parent->Lookup(name, &vn); status != ZX_OK) {
    *out = nullptr;
    return;
  }
  ASSERT_TRUE(vn);
  ASSERT_EQ(vn->Open(nullptr), ZX_OK);
  *out = std::move(vn);
}

void FileTester::CreateChild(Dir *vn, umode_t mode, std::string_view name) {
  zx::result tmp_child = vn->CreateWithMode(name, mode);
  ASSERT_TRUE(tmp_child.is_ok()) << tmp_child.status_string();
  ASSERT_EQ(tmp_child->Close(), ZX_OK);
}

void FileTester::DeleteChild(Dir *vn, std::string_view name, bool is_dir) {
  ASSERT_EQ(vn->Unlink(name, is_dir), ZX_OK);
  // TODO: After EvictInode available, check if nids of the child are correctly freed
}

void FileTester::RenameChild(fbl::RefPtr<Dir> &old_vnode, fbl::RefPtr<Dir> &new_vnode,
                             std::string_view oldname, std::string_view newname) {
  ASSERT_EQ(old_vnode->Rename(new_vnode, oldname, newname, false, false), ZX_OK);
}

void FileTester::CreateChildren(F2fs *fs, std::vector<fbl::RefPtr<VnodeF2fs>> &vnodes,
                                std::vector<uint32_t> &inos, fbl::RefPtr<Dir> &parent,
                                std::string name, size_t inode_cnt) {
  for (uint32_t i = 0; i < inode_cnt; ++i) {
    std::string file_name = name + std::to_string(i);
    zx::result test_file = parent->Create(file_name, fs::CreationType::kFile);
    ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
    fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));

    inos.push_back(test_file_vn->Ino());
    vnodes.push_back(std::move(test_file_vn));
  }
}

void FileTester::DeleteChildren(std::vector<fbl::RefPtr<VnodeF2fs>> &vnodes,
                                fbl::RefPtr<Dir> &parent, size_t inode_cnt) {
  uint32_t deleted_file_cnt = 0;
  for (const auto &iter : vnodes) {
    ASSERT_EQ(parent->Unlink(iter->GetNameView(), false), ZX_OK);
    ++deleted_file_cnt;
  }
  ASSERT_EQ(deleted_file_cnt, inode_cnt);
}

void FileTester::VnodeWithoutParent(F2fs *fs, umode_t mode,
                                    fbl::RefPtr<VnodeF2fs> &vnode) TA_NO_THREAD_SAFETY_ANALYSIS {
  nid_t inode_nid;
  auto nid_or = fs->GetNodeManager().AllocNid();
  ASSERT_TRUE(nid_or.is_ok());
  inode_nid = *nid_or;

  if (S_ISDIR(mode)) {
    vnode = fbl::MakeRefCounted<Dir>(fs, inode_nid, mode);
  } else {
    vnode = fbl::MakeRefCounted<File>(fs, inode_nid, mode);
  }
  vnode->ClearFlag(InodeInfoFlag::kNewInode);
  ASSERT_EQ(vnode->Open(nullptr), ZX_OK);
  fs->GetVCache().Add(vnode.get());
  vnode->ClearLinkCount();
  vnode->SetOrphan();
}

void FileTester::CheckInlineDir(VnodeF2fs *vn) {
  ASSERT_NE(vn->TestFlag(InodeInfoFlag::kInlineDentry), false);
  ASSERT_EQ(vn->GetSize(), vn->MaxInlineData());
}

void FileTester::CheckNonInlineDir(VnodeF2fs *vn) {
  ASSERT_EQ(vn->TestFlag(InodeInfoFlag::kInlineDentry), false);
  ASSERT_GT(vn->GetSize(), vn->MaxInlineData());
}

void FileTester::CheckInlineFile(VnodeF2fs *vn) {
  ASSERT_NE(vn->TestFlag(InodeInfoFlag::kInlineData), false);
}

void FileTester::CheckNonInlineFile(VnodeF2fs *vn) {
  ASSERT_EQ(vn->TestFlag(InodeInfoFlag::kInlineData), false);
}

void FileTester::CheckDataExistFlagSet(VnodeF2fs *vn) {
  ASSERT_NE(vn->TestFlag(InodeInfoFlag::kDataExist), false);
}

void FileTester::CheckDataExistFlagUnset(VnodeF2fs *vn) {
  ASSERT_EQ(vn->TestFlag(InodeInfoFlag::kDataExist), false);
}

void FileTester::CheckInlineXattr(VnodeF2fs *vn) {
  ASSERT_NE(vn->TestFlag(InodeInfoFlag::kInlineXattr), false);
}

void FileTester::CheckChildrenFromReaddir(Dir *dir, std::unordered_set<std::string> childs) {
  childs.insert(".");

  fs::VdirCookie cookie;
  uint8_t buf[kBlockSize];
  size_t len;

  ASSERT_EQ(dir->Readdir(&cookie, buf, sizeof(buf), &len), ZX_OK);

  uint8_t *buf_ptr = buf;

  while (len > 0 && buf_ptr < buf + kBlockSize) {
    auto entry = reinterpret_cast<const fs::DirectoryEntry *>(buf_ptr);
    size_t entry_size = entry->name_length + sizeof(fs::DirectoryEntry);

    std::string_view entry_name(entry->name, entry->name_length);
    auto iter = childs.begin();
    for (; iter != childs.end(); ++iter) {
      if (entry_name == *iter) {
        break;
      }
    }

    ASSERT_NE(iter, childs.end());
    childs.erase(iter);

    buf_ptr += entry_size;
    len -= entry_size;
  }

  ASSERT_TRUE(childs.empty());
}

void FileTester::CheckChildrenInBlock(Dir *vn, uint64_t bidx,
                                      std::unordered_set<std::string> childs) {
  if (bidx == 0) {
    childs.insert(".");
    childs.insert("..");
  }

  if (childs.empty()) {
    ASSERT_EQ(vn->FindDataPage(bidx).status_value(), ZX_ERR_NOT_FOUND);
    return;
  }

  auto page_or = vn->FindDataPage(bidx);
  ZX_ASSERT(page_or.is_ok());
  DentryBlock *dentry_blk = page_or->GetAddress<DentryBlock>();
  PageBitmap dentry_bitmap(dentry_blk->dentry_bitmap, kNrDentryInBlock);

  size_t bit_pos = 0;
  while ((bit_pos = dentry_bitmap.FindNextBit(bit_pos)) < kNrDentryInBlock) {
    DirEntry *de = &dentry_blk->dentry[bit_pos];
    uint32_t slots = (LeToCpu(de->name_len) + kNameLen - 1) / kNameLen;

    std::string_view dir_entry_name(reinterpret_cast<char *>(dentry_blk->filename[bit_pos]),
                                    LeToCpu(de->name_len));
    auto iter = childs.begin();
    for (; iter != childs.end(); ++iter) {
      if (dir_entry_name == *iter) {
        break;
      }
    }

    ASSERT_NE(iter, childs.end());
    childs.erase(iter);

    bit_pos += slots;
  }

  ASSERT_TRUE(childs.empty());
}

std::string FileTester::GetRandomName(unsigned int len) {
  const char *char_list = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  auto char_list_len = strlen(char_list);
  auto generator = [&]() { return char_list[rand() % char_list_len]; };
  std::string str(len, 0);
  std::generate_n(str.begin(), len, generator);
  return str;
}

void FileTester::AppendToInline(File *file, const void *data, size_t len) {
  size_t offset = file->GetSize();
  size_t ret = 0;
  if (file->TestFlag(InodeInfoFlag::kInlineData) && offset + len < file->MaxInlineData()) {
    ASSERT_EQ(file->WriteInline(data, len, offset, &ret), ZX_OK);
    file->SetSize(std::max(file->GetSize(), offset + len));
    ASSERT_EQ(ret, len);
  }
}

void FileTester::AppendToFile(File *file, const void *data, size_t len) {
  size_t actual;
  size_t end;
  ASSERT_EQ(Append(file, data, len, &end, &actual), ZX_OK);
  ASSERT_EQ(actual, len);
}

void FileTester::ReadFromFile(File *file, void *data, size_t len, size_t off) {
  size_t actual;
  ASSERT_EQ(Read(file, data, len, off, &actual), ZX_OK);
  ASSERT_EQ(actual, len);
}

zx_status_t FileTester::Read(File *file, void *data, size_t len, size_t off, size_t *out_actual) {
  zx::result<zx::stream> stream = file->CreateStream(ZX_STREAM_MODE_READ);
  if (stream.is_error()) {
    return stream.error_value();
  }
  zx_iovec_t iov = {
      .buffer = data,
      .capacity = len,
  };
  return stream->readv_at(0, off, &iov, 1, out_actual);
}

zx_status_t FileTester::Write(File *file, const void *data, size_t len, size_t offset,
                              size_t *out_actual) {
  zx::result<zx::stream> stream = file->CreateStream(ZX_STREAM_MODE_WRITE);
  if (stream.is_error()) {
    return stream.error_value();
  }
  // Since zx_iovec_t::buffer is not a const type, we make a copied buffer and use it.
  auto copied = std::make_unique<uint8_t[]>(len);
  std::memcpy(copied.get(), data, len);
  zx_iovec_t iov = {
      .buffer = copied.get(),
      .capacity = len,
  };
  return stream->writev_at(0, offset, &iov, 1, out_actual);
}

zx_status_t FileTester::Append(File *file, const void *data, size_t len, size_t *out_end,
                               size_t *out_actual) {
  *out_end = file->GetSize();
  return Write(file, data, len, *out_end, out_actual);
}

void MapTester::CheckNodeLevel(F2fs *fs, VnodeF2fs *vn, uint32_t level) {
  LockedPage ipage;
  ASSERT_EQ(fs->GetNodeManager().GetNodePage(vn->Ino(), &ipage), ZX_OK);
  Inode &inode = ipage->GetAddress<Node>()->i;

  uint32_t i;
  for (i = 0; i < level; ++i)
    ASSERT_NE(inode.i_nid[i], 0U);

  for (; i < kNidsPerInode; ++i)
    ASSERT_EQ(inode.i_nid[i], 0U);
}

void MapTester::CheckNidsFree(F2fs *fs, std::unordered_set<nid_t> &nids) {
  NodeManager &nm_i = fs->GetNodeManager();

  std::lock_guard lock(nm_i.free_nid_tree_lock_);
  for (auto nid : nids) {
    bool found = false;
    for (const auto &iter : nm_i.free_nid_tree_) {
      if (iter == nid) {
        found = true;
        break;
      }
    }
    ASSERT_TRUE(found);
  }
}

void MapTester::CheckNidsInuse(F2fs *fs, std::unordered_set<nid_t> &nids) {
  NodeManager &nm_i = fs->GetNodeManager();

  std::lock_guard lock(nm_i.free_nid_tree_lock_);
  for (auto nid : nids) {
    bool found = false;
    for (const auto &iter : nm_i.free_nid_tree_) {
      if (iter == nid) {
        found = true;
        break;
      }
    }
    ASSERT_FALSE(found);
  }
}

void MapTester::CheckBlkaddrsFree(F2fs *fs, std::unordered_set<block_t> &blkaddrs) {
  SuperblockInfo &superblock_info = fs->GetSuperblockInfo();
  for (auto blkaddr : blkaddrs) {
    SegmentManager &manager = fs->GetSegmentManager();
    const SegmentEntry &se = manager.GetSegmentEntry(manager.GetSegmentNumber(blkaddr));
    uint32_t offset = manager.GetSegOffFromSeg0(blkaddr) & (superblock_info.GetBlocksPerSeg() - 1);
    ASSERT_EQ(se.ckpt_valid_map.GetOne(ToMsbFirst(offset)), false);
  }
}

void MapTester::CheckBlkaddrsInuse(F2fs *fs, std::unordered_set<block_t> &blkaddrs) {
  SuperblockInfo &superblock_info = fs->GetSuperblockInfo();
  for (auto blkaddr : blkaddrs) {
    SegmentManager &manager = fs->GetSegmentManager();
    const SegmentEntry &se = manager.GetSegmentEntry(manager.GetSegmentNumber(blkaddr));
    uint32_t offset = manager.GetSegOffFromSeg0(blkaddr) & (superblock_info.GetBlocksPerSeg() - 1);
    ASSERT_NE(se.ckpt_valid_map.GetOne(ToMsbFirst(offset)), false);
  }
}

void MapTester::CheckDnodePage(NodePage &page, nid_t exp_nid) {
  ASSERT_EQ(page.NidOfNode(), exp_nid);
  ASSERT_EQ(page.GetBlockAddr(1U), 0U);
}

bool MapTester::IsCachedNat(NodeManager &node_manager, nid_t n) {
  fs::SharedLock nat_lock(node_manager.nat_tree_lock_);
  auto entry = node_manager.nat_cache_.find(n);
  return entry != node_manager.nat_cache_.end();
}

void MapTester::RemoveTruncatedNode(NodeManager &node_manager, std::vector<nid_t> &nids) {
  fs::SharedLock nat_lock(node_manager.nat_tree_lock_);
  for (auto iter = nids.begin(); iter != nids.end();) {
    auto cache_entry = node_manager.nat_cache_.find(*iter);
    if (cache_entry != node_manager.nat_cache_.end()) {
      if ((*cache_entry).GetBlockAddress() == kNullAddr) {
        iter = nids.erase(iter);
      } else {
        ++iter;
      }
    }
  }
}

void MapTester::DoWriteNat(F2fs *fs, nid_t nid, block_t blkaddr, uint8_t version) {
  NodeManager *nm_i = &fs->GetNodeManager();
  std::unique_ptr<NatEntry> nat_entry = std::make_unique<NatEntry>();
  auto cache_entry = nat_entry.get();

  cache_entry->SetNid(nid);

  ZX_ASSERT(!(*cache_entry).fbl::WAVLTreeContainable<std::unique_ptr<NatEntry>>::InContainer());

  std::lock_guard nat_lock(nm_i->nat_tree_lock_);
  nm_i->nat_cache_.insert(std::move(nat_entry));

  ZX_ASSERT(!(*cache_entry).fbl::DoublyLinkedListable<NatEntry *>::InContainer());
  nm_i->clean_nat_list_.push_back(cache_entry);
  ++nm_i->nat_entries_count_;

  cache_entry->ClearCheckpointed();
  cache_entry->SetBlockAddress(blkaddr);
  cache_entry->SetVersion(version);
  ZX_ASSERT((*cache_entry).fbl::DoublyLinkedListable<NatEntry *>::InContainer());
  nm_i->clean_nat_list_.erase(*cache_entry);
  ZX_ASSERT(!(*cache_entry).fbl::DoublyLinkedListable<NatEntry *>::InContainer());
  nm_i->dirty_nat_list_.push_back(cache_entry);
}

void MapTester::DoWriteSit(F2fs *fs, CursegType type, uint32_t exp_segno,
                           block_t *new_blkaddr) TA_NO_THREAD_SAFETY_ANALYSIS {
  SuperblockInfo &superblock_info = fs->GetSuperblockInfo();
  SegmentManager &segment_manager = fs->GetSegmentManager();

  if (!segment_manager.HasCursegSpace(type)) {
    segment_manager.AllocateSegmentByDefault(type, false);
  }

  CursegInfo *curseg = segment_manager.CURSEG_I(type);
  if (exp_segno != kNullSegNo) {
    ASSERT_EQ(curseg->segno, exp_segno);
  }

  std::lock_guard curseg_lock(curseg->curseg_mutex);
  *new_blkaddr = segment_manager.NextFreeBlkAddr(type);
  uint32_t old_cursegno = curseg->segno;

  segment_manager.RefreshNextBlkoff(curseg);
  superblock_info.IncBlockCount(curseg->alloc_type);

  segment_manager.RefreshSitEntry(kNullSegNo, *new_blkaddr);
  segment_manager.LocateDirtySegment(old_cursegno);

  if (!segment_manager.HasCursegSpace(type)) {
    segment_manager.AllocateSegmentByDefault(type, false);
  }
}

void MapTester::RemoveAllNatEntries(NodeManager &manager) {
  std::lock_guard nat_lock(manager.nat_tree_lock_);
  for (auto &nat_entry : manager.nat_cache_) {
    ZX_ASSERT((nat_entry).fbl::DoublyLinkedListable<NatEntry *>::InContainer());
    manager.clean_nat_list_.erase(nat_entry);
    ZX_ASSERT((nat_entry).fbl::WAVLTreeContainable<std::unique_ptr<NatEntry>>::InContainer());
    --manager.nat_entries_count_;
  }
  manager.nat_cache_.clear();
}

nid_t MapTester::ScanFreeNidList(NodeManager &manager) {
  std::lock_guard free_nid_lock(manager.free_nid_tree_lock_);
  return manager.free_nid_tree_.empty() ? 0 : *manager.free_nid_tree_.rbegin();
}

void MapTester::GetCachedNatEntryBlockAddress(NodeManager &manager, nid_t nid, block_t &out) {
  fs::SharedLock nat_lock(manager.nat_tree_lock_);
  auto entry = manager.nat_cache_.find(nid);
  ASSERT_TRUE(entry != manager.nat_cache_.end());
  ASSERT_EQ(entry->GetNodeInfo().nid, nid);
  out = entry->GetBlockAddress();
}

void MapTester::SetCachedNatEntryBlockAddress(NodeManager &manager, nid_t nid, block_t address) {
  std::lock_guard nat_lock(manager.nat_tree_lock_);
  auto entry = manager.nat_cache_.find(nid);
  ASSERT_TRUE(entry != manager.nat_cache_.end());
  ASSERT_EQ(entry->GetNodeInfo().nid, nid);
  entry->SetBlockAddress(address);
}

void MapTester::SetCachedNatEntryCheckpointed(NodeManager &manager, nid_t nid) {
  std::lock_guard nat_lock(manager.nat_tree_lock_);
  auto entry = manager.nat_cache_.find(nid);
  ASSERT_TRUE(entry != manager.nat_cache_.end());
  ASSERT_EQ(entry->GetNodeInfo().nid, nid);
  entry->SetCheckpointed();
  ASSERT_TRUE(entry->IsCheckpointed());
}

zx_status_t MkfsTester::InitAndGetDeviceInfo(MkfsWorker &mkfs) {
  mkfs.InitGlobalParameters();
  return mkfs.GetDeviceInfo();
}

zx::result<std::unique_ptr<BcacheMapper>> MkfsTester::FormatDevice(MkfsWorker &mkfs) {
  if (zx_status_t ret = mkfs.FormatDevice(); ret != ZX_OK)
    return zx::error(ret);
  return zx::ok(std::move(mkfs.bc_));
}

zx_status_t GcTester::DoGarbageCollect(SegmentManager &manager, uint32_t segno, GcType gc_type) {
  std::lock_guard lock(f2fs::GetGlobalLock());
  return manager.DoGarbageCollect(segno, gc_type);
}

zx_status_t GcTester::GcDataSegment(SegmentManager &manager, const SummaryBlock &sum_blk,
                                    unsigned int segno, GcType gc_type) {
  std::lock_guard lock(f2fs::GetGlobalLock());
  return manager.GcDataSegment(sum_blk, segno, gc_type);
}

void DeviceTester::SetHook(F2fs *fs, DeviceTester::Hook hook) {
  fs->GetBc().ForEachBcache([](Bcache *f2fs_device) {
    static_cast<block_client::FakeBlockDevice *>(f2fs_device->GetDevice())->Pause();
  });
  fs->GetBc().ForEachBcache([hook](Bcache *f2fs_device) {
    static_cast<block_client::FakeBlockDevice *>(f2fs_device->GetDevice())->set_hook(hook);
  });
  fs->GetBc().ForEachBcache([](Bcache *f2fs_device) {
    static_cast<block_client::FakeBlockDevice *>(f2fs_device->GetDevice())->Resume();
  });
}

}  // namespace f2fs
