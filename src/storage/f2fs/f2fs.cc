// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/f2fs.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/event.h>
#include <sys/stat.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <safemath/checked_math.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/dir.h"
#include "src/storage/f2fs/file.h"
#include "src/storage/f2fs/file_cache.h"
#include "src/storage/f2fs/inspect.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/reader.h"
#include "src/storage/f2fs/segment.h"
#include "src/storage/f2fs/storage_buffer.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vnode.h"
#include "src/storage/f2fs/vnode_cache.h"
#include "src/storage/f2fs/writeback.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

static zx_status_t CheckBlockSize(const Superblock& sb) {
  if (kF2fsSuperMagic != LeToCpu(sb.magic)) {
    return ZX_ERR_INVALID_ARGS;
  }

  block_t blocksize =
      safemath::CheckLsh<block_t>(1, LeToCpu(sb.log_blocksize)).ValueOrDefault(kUint32Max);
  if (blocksize != kBlockSize)
    return ZX_ERR_INVALID_ARGS;
  // 512/1024/2048/4096 sector sizes are supported.
  if (LeToCpu(sb.log_sectorsize) > kMaxLogSectorSize ||
      LeToCpu(sb.log_sectorsize) < kMinLogSectorSize)
    return ZX_ERR_INVALID_ARGS;
  if ((LeToCpu(sb.log_sectors_per_block) + LeToCpu(sb.log_sectorsize)) != kMaxLogSectorSize)
    return ZX_ERR_INVALID_ARGS;
  if (LeToCpu(sb.log_blocks_per_seg) != kDefaultLogBlocksPerSegment)
    return ZX_ERR_INVALID_ARGS;

  uint32_t segs_per_sec = LeToCpu(sb.segs_per_sec);
  uint32_t secs_per_zone = LeToCpu(sb.secs_per_zone);
  uint32_t segment_count_main = LeToCpu(sb.segment_count_main);
  uint32_t section_count = LeToCpu(sb.section_count);

  if (segs_per_sec == 0 || secs_per_zone == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (segment_count_main % segs_per_sec != 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (section_count != segment_count_main / segs_per_sec) {
    return ZX_ERR_INVALID_ARGS;
  }

  uint32_t segment_count = LeToCpu(sb.segment_count);
  uint32_t segment_count_ckpt = LeToCpu(sb.segment_count_ckpt);
  uint32_t segment_count_sit = LeToCpu(sb.segment_count_sit);
  uint32_t segment_count_nat = LeToCpu(sb.segment_count_nat);
  uint32_t segment_count_ssa = LeToCpu(sb.segment_count_ssa);

  if (segment_count_ckpt < 2 || segment_count_sit < 2 || (segment_count_sit % 2 != 0) ||
      segment_count_nat < 2 || (segment_count_nat % 2 != 0) || segment_count_ssa == 0 ||
      segment_count_main == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto checked_meta_segments = safemath::CheckAdd<uint32_t>(segment_count_ckpt, segment_count_sit,
                                                            segment_count_nat, segment_count_ssa);
  if (!checked_meta_segments.IsValid()) {
    return ZX_ERR_INVALID_ARGS;
  }
  auto checked_total_segments = checked_meta_segments + segment_count_main;
  if (!checked_total_segments.IsValid() || checked_total_segments.ValueOrDie() > segment_count) {
    return ZX_ERR_INVALID_ARGS;
  }

  const uint32_t cp_payload = LeToCpu(sb.cp_payload);
  const uint64_t sit_ver_bitmap_bytesize =
      VersionBitmapByteSize(segment_count_sit, LeToCpu(sb.log_blocks_per_seg));
  const uint64_t nat_ver_bitmap_bytesize =
      VersionBitmapByteSize(segment_count_nat, LeToCpu(sb.log_blocks_per_seg));
  if (cp_payload > kMaxCpPayload || sit_ver_bitmap_bytesize > kMaxSitBitmapSize ||
      nat_ver_bitmap_bytesize > kMaxBitmapBytes) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (cp_payload == 0) {
    if (sit_ver_bitmap_bytesize + nat_ver_bitmap_bytesize > kMaxBitmapBytes) {
      return ZX_ERR_INVALID_ARGS;
    }
  } else {
    if (sit_ver_bitmap_bytesize > static_cast<uint64_t>(cp_payload) * kBlockSize) {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  return ZX_OK;
}

zx::result<std::unique_ptr<Superblock>> LoadSuperblock(BcacheMapper& bc) {
  BlockBuffer block;
  constexpr int kSuperblockCount = 2;
  zx_status_t status;
  for (auto i = 0; i < kSuperblockCount; ++i) {
    if (status = bc.Readblk(kSuperblockStart + i, block.get()); status != ZX_OK) {
      continue;
    }
    auto superblock = std::make_unique<Superblock>();
    std::memcpy(superblock.get(), block.get<uint8_t>() + kSuperOffset, sizeof(Superblock));
    if (status = CheckBlockSize(*superblock); status != ZX_OK) {
      continue;
    }
    return zx::ok(std::move(superblock));
  }
  FX_LOGS(ERROR) << "failed to read superblock." << status;
  return zx::error(status);
}

F2fs::F2fs(FuchsiaDispatcher dispatcher, std::unique_ptr<f2fs::BcacheMapper> bc,
           const MountOptions& mount_options, PlatformVfs* vfs)
    : dispatcher_(dispatcher), vfs_(vfs), bc_(std::move(bc)), mount_options_(mount_options) {
  inspect_tree_ = std::make_unique<InspectTree>(this);
  vnode_cache_ = std::make_unique<VnodeCache>();
  zx::event::create(0, &fs_id_);
}

void F2fs::StartMemoryPressureWatcher() {
  if (dispatcher_) {
    memory_pressure_watcher_ =
        std::make_unique<MemoryPressureWatcher>(dispatcher_, [this](MemoryPressure level) {
          // release-acquire ordering with GetMemoryStatus()
          current_memory_pressure_level_.store(level, std::memory_order_release);
          ScheduleWritebackAndReclaimPages();
          FX_LOGS(INFO) << "Memory pressure level: " << MemoryPressureWatcher::ToString(level);
        });
  }
}

zx::result<std::unique_ptr<F2fs>> F2fs::Create(FuchsiaDispatcher dispatcher,
                                               std::unique_ptr<f2fs::BcacheMapper> bc,
                                               const MountOptions& options, PlatformVfs* vfs) {
  // Make sure blocks are page-aligned.
  const size_t page_size = zx_system_get_page_size();
  ZX_ASSERT_MSG(kBlockSize % page_size == 0, "Blocks are not page-aligned when page size is %#zx",
                page_size);

  zx::result<std::unique_ptr<Superblock>> superblock;
  if (superblock = LoadSuperblock(*bc); superblock.is_error()) {
    return superblock.take_error();
  }

  auto fs = std::make_unique<F2fs>(dispatcher, std::move(bc), options, vfs);
  if (zx_status_t status = fs->LoadSuper(std::move(*superblock)); status != ZX_OK) {
    FX_LOGS(ERROR) << "failed to initialize fs." << status;
    return zx::error(status);
  }
  fs->StartMemoryPressureWatcher();
  return zx::ok(std::move(fs));
}

void F2fs::Sync(SyncCallback closure) {
  SyncFs(true);
  if (closure) {
    closure(ZX_OK);
  }
}

zx::result<fs::FilesystemInfo> F2fs::GetFilesystemInfo() {
  fs::FilesystemInfo info;
  info.block_size = kBlockSize;
  info.max_filename_size = kMaxNameLen;
  info.fs_type = fuchsia_fs::VfsType::kF2Fs;
  info.total_bytes =
      safemath::CheckMul<uint64_t>(superblock_info_->GetTotalBlockCount(), kBlockSize).ValueOrDie();
  info.used_bytes =
      safemath::CheckMul<uint64_t>(superblock_info_->GetValidBlockCount(), kBlockSize).ValueOrDie();
  info.total_nodes = superblock_info_->GetMaxNodeCount();
  info.used_nodes = superblock_info_->GetValidInodeCount();
  info.SetFsId(fs_id_);
  info.name = "f2fs";
  info.free_shared_pool_bytes = fbl::round_down(
      std::min(bc_->GetFreeSpaceSize(), superblock_info_->GetFreeBlockCount() * kBlockSize),
      bc_->GetSliceSize());

  return zx::ok(info);
}

bool F2fs::IsValid() const {
  if (bc_ == nullptr) {
    return false;
  }
  if (root_vnode_ == nullptr) {
    return false;
  }
  if (superblock_info_ == nullptr) {
    return false;
  }
  if (segment_manager_ == nullptr) {
    return false;
  }
  if (node_manager_ == nullptr) {
    return false;
  }
  return true;
}

// Fill the locked page with data located in the block address.
zx::result<> F2fs::MakeReadOperation(LockedPage& page, block_t blk_addr, PageType type,
                                     bool is_sync) {
  if (page->IsUptodate()) {
    return zx::ok();
  }
  std::vector<block_t> addrs = {blk_addr};
  std::vector<LockedPage> pages;
  pages.push_back(std::move(page));
  auto status = MakeReadOperations(pages, addrs, type, is_sync);
  if (status.is_error()) {
    return status.take_error();
  }
  page = std::move(pages[0]);
  return zx::ok();
}

zx::result<> F2fs::MakeReadOperations(zx::vmo& vmo, std::vector<block_t>& addrs, PageType type,
                                      bool is_sync) {
  auto ret = reader_->ReadBlocks(vmo, addrs);
  if (ret.is_error()) {
    FX_LOGS(WARNING) << "failed to read blocks. " << ret.status_string();
    if (ret.status_value() == ZX_ERR_UNAVAILABLE || ret.status_value() == ZX_ERR_PEER_CLOSED) {
      // The underlying block device is unavailable. Set kCpErrorFlag for f2fs to enter read-only
      // mode.
      superblock_info_->SetCpFlags(CpFlag::kCpErrorFlag);
    }
  }
  return ret;
}

zx::result<> F2fs::MakeReadOperations(std::vector<LockedPage>& pages, std::vector<block_t>& addrs,
                                      PageType type, bool is_sync) {
  auto ret = reader_->ReadBlocks(pages, addrs);
  if (ret.is_error()) {
    FX_LOGS(WARNING) << "failed to read blocks. " << ret.status_string();
    if (ret.status_value() == ZX_ERR_UNAVAILABLE || ret.status_value() == ZX_ERR_PEER_CLOSED) {
      // The underlying block device is unavailable. Set kCpErrorFlag for f2fs to enter read-only
      // mode.
      superblock_info_->SetCpFlags(CpFlag::kCpErrorFlag);
    }
  }
  return ret;
}

zx_status_t F2fs::MakeTrimOperation(block_t blk_addr, block_t nblocks) const {
  return GetBc().Trim(blk_addr, nblocks);
}

void F2fs::PutSuper() {
#if 0  // porting needed
  // DestroyStats(superblock_info_.get());
  // StopGcThread(superblock_info_.get());
#endif
  SetTearDown();
  writer_.reset();
  reader_.reset();
  GetVCache().Reset();
}

void F2fs::ScheduleWritebackAndReclaimPages() {
  // Schedule a writeback task for kernel to reclaim pager-backed/discardable VMOs until memory
  // pressure gets lower. It ensures that neither checkpoint nor gc runs during writeback. If there
  // is not enough space, it stops to avoid out of space.
  if (GetMemoryStatus(MemoryStatus::kNeedReclaim)) {
    auto promise = fpromise::make_promise([this]() __TA_EXCLUDES(writeback_mutex_) {
      std::lock_guard lock(writeback_mutex_);
      fs::SharedLock fs_lock(f2fs::GetGlobalLock());

      // For each iteration, it tries writeback for kDefaultBlocksPerSegment blocks, which can yield
      // n dirty node blocks in the worst case. The reserved space can cover the dirty node blocks.
      while (!segment_manager_->HasNotEnoughFreeSecs(0, kDefaultBlocksPerSegment) &&
             GetMemoryStatus(MemoryStatus::kNeedWriteback)) {
        GetVCache().ForDirtyVnodesIf(
            [&](fbl::RefPtr<VnodeF2fs>& vnode) TA_NO_THREAD_SAFETY_ANALYSIS {
              size_t num_data_blocks =
                  std::min(vnode->GetDirtyPageCount(), kDefaultBlocksPerSegment);
              if (segment_manager_->HasNotEnoughFreeSecs(0, num_data_blocks)) {
                return ZX_ERR_STOP;
              }
              WritebackOperation op = {.to_write = num_data_blocks, .bReclaim = true};
              vnode->Writeback(op);
              return ZX_OK;
            },
            [](fbl::RefPtr<VnodeF2fs>& vnode) {
              if (!vnode->IsDir() && vnode->GetDirtyPageCount()) {
                return ZX_OK;
              }
              return ZX_ERR_NEXT;
            });
      }
      // If memory pressure is high even after data writeback, do checkpoint to clear inactive
      // vnodes and to unlock discardable VMOs.
      if (GetMemoryStatus(MemoryStatus::kNeedCheckpoint)) {
        fs_lock.unlock();
        WriteCheckpoint(false);
      }
      memory_cvar_.notify_all();
      return fpromise::ok();
    });
    writer_->ScheduleWriteback(std::move(promise));
  }
}

zx_status_t F2fs::SyncFs(bool bShutdown) {
  std::lock_guard lock(f2fs::GetGlobalLock());
  return SyncFsUnsafe(bShutdown);
}
zx_status_t F2fs::SyncFsUnsafe(bool bShutdown) {
  // TODO:: Consider !superblock_info_.IsDirty()
  if (bShutdown) {
    FX_LOGS(INFO) << "prepare for shutdown";
    // Stop listening to memorypressure.
    memory_pressure_watcher_.reset();
    // Clear the dirty flag of orphans to avoid unnecessary block writes.
    GetVCache().ForDirtyVnodesIf(
        [&](fbl::RefPtr<VnodeF2fs>& vnode) {
          LockedPage page;
          vnode->ClearDirty();
          vnode->SetFlag(InodeInfoFlag::kNoAlloc);
          if (zx_status_t ret = GetNodeManager().GetNodePage(vnode->GetKey(), &page);
              ret != ZX_OK) {
            return ZX_ERR_NEXT;
          }
          vnode->UpdateInodePage(page, true);
          return ZX_OK;
        },
        [](fbl::RefPtr<VnodeF2fs>& vnode) {
          if (!vnode->GetLinkCount()) {
            return ZX_OK;
          }
          return ZX_ERR_NEXT;
        });
    // Flush every dirty Pages.
    do {
      // If CpFlag::kCpErrorFlag is set, it cannot be synchronized to disk. So we will drop all
      // dirty pages.
      if (superblock_info_->TestCpFlags(CpFlag::kCpErrorFlag)) {
        return ZX_ERR_INTERNAL;
      }
      WritebackOperation op = {.to_write = kDefaultBlocksPerSegment, .bReclaim = true};
      op.if_vnode = [](fbl::RefPtr<VnodeF2fs>& vnode) {
        if ((!vnode->IsDir() && vnode->GetDirtyPageCount())) {
          return ZX_OK;
        }
        return ZX_ERR_NEXT;
      };

      uint32_t to_write =
          std::min(static_cast<uint32_t>(superblock_info_->GetPageCount(CountType::kDirtyData)),
                   kDefaultBlocksPerSegment);
      if (to_write) {
        AllocateFreeSections(to_write);
        FlushDirtyDataPages(op);
      }
    } while (superblock_info_->GetPageCount(CountType::kDirtyData));
    FX_LOGS(INFO) << "flushing dirty pages… done";
  }
  return WriteCheckpointUnsafe(bShutdown);
}

#if 0  // porting needed
// int F2fs::F2fsStatfs(dentry *dentry /*, kstatfs *buf*/) {
  // super_block *sb = dentry->d_sb;
  // SuperblockInfo *superblock_info = F2FS_SB(sb);
  // u64 id = huge_encode_dev(sb->s_bdev->bd_dev);
  // block_t total_count, user_block_count, start_count, ovp_count;

  // total_count = LeToCpu(superblock_info->raw_super->block_count);
  // user_block_count = superblock_info->GetTotalBlockCount();
  // start_count = LeToCpu(superblock_info->raw_super->segment0_blkaddr);
  // ovp_count = GetSmInfo(superblock_info).ovp_segments << superblock_info->GetLogBlocksPerSeg();
  // buf->f_type = kF2fsSuperMagic;
  // buf->f_bsize = superblock_info->GetBlocksize();

  // buf->f_blocks = total_count - start_count;
  // buf->f_bfree = buf->f_blocks - .GetValidBlockCount(superblock_info) - ovp_count;
  // buf->f_bavail = user_block_count - .GetValidBlockCount(superblock_info);

  // buf->f_files = ValidInodeCount(superblock_info);
  // buf->f_ffree = superblock_info->GetTotalNodeCount() - ValidNodeCount(superblock_info);

  // buf->f_namelen = kMaxNameLen;
  // buf->f_fsid.val[0] = (u32)id;
  // buf->f_fsid.val[1] = (u32)(id >> 32);

  // return 0;
// }

// VnodeF2fs *F2fs::F2fsNfsGetInode(uint64_t ino, uint32_t generation) {
//   fbl::RefPtr<VnodeF2fs> vnode_refptr;
//   VnodeF2fs *vnode = nullptr;
//   int err;

//   if (ino < superblock_info_->GetRootIno())
//     return (VnodeF2fs *)ErrPtr(-ESTALE);

//   /*
//    * f2fs_iget isn't quite right if the inode is currently unallocated!
//    * However f2fs_iget currently does appropriate checks to handle stale
//    * inodes so everything is OK.
//    */
//   err = GetVnode(ino, &vnode_refptr);
//   if (err)
//     return (VnodeF2fs *)ErrPtr(err);
//   vnode = vnode_refptr.get();
//   if (generation && vnode->i_generation != generation) {
//     /* we didn't find the right inode.. */
//     return (VnodeF2fs *)ErrPtr(-ESTALE);
//   }
//   return vnode;
// }

// struct fid {};

// dentry *F2fs::F2fsFhToDentry(fid *fid, int fh_len, int fh_type) {
//   return generic_fh_to_dentry(sb, fid, fh_len, fh_type,
//             f2fs_nfs_get_inode);
// }

// dentry *F2fs::F2fsFhToParent(fid *fid, int fh_len, int fh_type) {
//   return generic_fh_to_parent(sb, fid, fh_len, fh_type,
//             f2fs_nfs_get_inode);
// }
#endif

void F2fs::Reset() {
  GetVCache().Reset();
  root_vnode_.reset();
  meta_vnode_.reset();
  node_vnode_.reset();
  node_manager_.reset();
  if (segment_manager_) {
    segment_manager_->DestroySegmentManager();
    segment_manager_.reset();
  }
  superblock_info_.reset();
}

zx_status_t F2fs::LoadSuper(std::unique_ptr<Superblock> sb) {
  auto reset = fit::defer([&] { Reset(); });

  // allocate memory for f2fs-specific super block info
  superblock_info_ = std::make_unique<SuperblockInfo>(std::move(sb), mount_options_);
  ClearOnRecovery();

  node_vnode_ = std::make_unique<VnodeF2fs>(this, superblock_info_->GetNodeIno(), 0);
  meta_vnode_ = std::make_unique<VnodeF2fs>(this, superblock_info_->GetMetaIno(), 0);

  reader_ = std::make_unique<Reader>(std::make_unique<StorageBufferPool>(bc_.get()));
  writer_ = std::make_unique<Writer>(std::make_unique<StorageBufferPool>(bc_.get()));
  if (zx_status_t err = GetValidCheckpoint(); err != ZX_OK) {
    return err;
  }

  segment_manager_ = std::make_unique<SegmentManager>(this);
  node_manager_ = std::make_unique<NodeManager>(this);
  if (zx_status_t err = segment_manager_->BuildSegmentManager(); err != ZX_OK) {
    return err;
  }

  if (zx_status_t err = node_manager_->BuildNodeManager(); err != ZX_OK) {
    return err;
  }

  if (zx_status_t err = PurgeOrphanInodes(); err != ZX_OK) {
    return err;
  }

  zx::result vnode = GetVnode(superblock_info_->GetRootIno());
  if (vnode.is_error()) {
    return vnode.status_value();
  }
  root_vnode_ = std::move(*vnode);

  // root vnode is corrupted
  if (!root_vnode_->IsDir() || !root_vnode_->GetBlockCount() || !root_vnode_->GetSize()) {
    return ZX_ERR_INTERNAL;
  }

  if (!superblock_info_->TestOpt(MountOption::kDisableRollForward)) {
    RecoverFsyncData();
  }

  // TODO(https://fxbug.dev/42070949): enable a thread for background gc
  // After POR, we can run background GC thread
  // err = StartGcThread(superblock_info);
  // if (err)
  //   goto fail;
  reset.cancel();
  return ZX_OK;
}

void F2fs::AddToVnodeSet(VnodeSet type, nid_t ino) {
  std::lock_guard lock(vnode_set_mutex_);
  uint32_t flag = 1 << static_cast<uint32_t>(type);
  auto& node = vnode_set_[ino];
  if (node & flag) {
    return;
  }
  node |= flag;
  ++vnode_set_size_[static_cast<size_t>(type)];
}
void F2fs::RemoveFromVnodeSet(VnodeSet type, nid_t ino) {
  std::lock_guard lock(vnode_set_mutex_);
  uint32_t flag = 1 << static_cast<uint32_t>(type);
  auto vnode = vnode_set_.find(ino);
  if (vnode == vnode_set_.end() || !(vnode->second & flag)) {
    return;
  }
  vnode->second &= ~flag;
  --vnode_set_size_[static_cast<uint32_t>(type)];
}
bool F2fs::FindVnodeSet(VnodeSet type, nid_t ino) {
  fs::SharedLock lock(vnode_set_mutex_);
  uint32_t flag = 1 << static_cast<uint32_t>(type);
  const auto vnode = vnode_set_.find(ino);
  if (vnode == vnode_set_.end()) {
    return false;
  }
  return vnode->second & flag;
}
size_t F2fs::GetVnodeSetSize(VnodeSet type) {
  fs::SharedLock lock(vnode_set_mutex_);
  return vnode_set_size_[static_cast<uint32_t>(type)];
}
void F2fs::ForAllVnodeSet(VnodeSet type, fit::function<void(nid_t)> callback) {
  fs::SharedLock lock(vnode_set_mutex_);
  uint32_t flag = 1 << static_cast<uint32_t>(type);
  for (const auto& nid : vnode_set_) {
    if (nid.second & flag) {
      callback(nid.first);
    }
  }
}
void F2fs::ClearVnodeSet() {
  std::lock_guard lock(vnode_set_mutex_);
  for (uint32_t i = 0; i < static_cast<uint32_t>(VnodeSet::kMax); ++i) {
    vnode_set_size_[i] = 0;
  }
  vnode_set_.clear();
}

zx::result<fbl::RefPtr<VnodeF2fs>> F2fs::GetVnode(ino_t ino, LockedPage* inode_page) {
  if (ino < superblock_info_->GetRootIno() || !node_manager_->CheckNidRange(ino)) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  fbl::RefPtr<VnodeF2fs> vnode;
  if (vnode_cache_->Lookup(ino, &vnode) == ZX_OK) {
    if (unlikely(vnode->TestFlag(InodeInfoFlag::kNewInode))) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    return zx::ok(std::move(vnode));
  }

  LockedPage node_page;
  auto move_page = fit::defer([&node_page, &inode_page] { *inode_page = std::move(node_page); });
  if (inode_page) {
    node_page = std::move(*inode_page);
  }
  if (!node_page) {
    move_page.cancel();
    if (zx_status_t status = node_manager_->GetNodePage(ino, &node_page); status != ZX_OK) {
      return zx::error(status);
    }
  }

  // Invalid vnodes are allowed only during recovery.
  if (!IsOnRecovery() && !LeToCpu(node_page->GetAddress<Node>()->i.i_links)) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  if (!IsValidInlineLayout(node_page->GetAddress<Node>()->i)) {
    FX_LOGS(WARNING) << "inode " << ino << " has an inconsistent inline layout";
    return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
  }

  // When multiple threads attempt to make reference ptrs for the same inode number, those that come
  // after the winner can retrieve |vnode| from the cache.
  if (vnode_cache_->Lookup(ino, &vnode) == ZX_OK) {
    return zx::ok(std::move(vnode));
  }

  umode_t mode = LeToCpu(node_page->GetAddress<Node>()->i.i_mode);
  ZX_DEBUG_ASSERT(node_page.GetPage<NodePage>().InoOfNode() == ino);
  if (S_ISDIR(mode)) {
    vnode = fbl::MakeRefCounted<Dir>(this, ino, mode, std::move(node_page));
  } else {
    vnode = fbl::MakeRefCounted<File>(this, ino, mode, std::move(node_page));
  }

  if (zx_status_t status = vnode_cache_->Add(vnode.get()); status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(vnode));
}

zx::result<fbl::RefPtr<VnodeF2fs>> F2fs::CreateNewVnode(umode_t mode, std::optional<gid_t> gid) {
  zx::result ino = node_manager_->AllocNid();
  if (ino.is_error()) {
    return ino.take_error();
  }

  fbl::RefPtr<VnodeF2fs> vnode;
  if (S_ISDIR(mode)) {
    vnode = fbl::MakeRefCounted<Dir>(this, *ino, mode, gid);
  } else {
    vnode = fbl::MakeRefCounted<File>(this, *ino, mode, gid);
  }

  vnode_cache_->Add(vnode.get());
  vnode->SetDirty();
  return zx::ok(std::move(vnode));
}

bool F2fs::GetMemoryStatus(MemoryStatus status) {
  // release-acquire ordering with MemoryPressureWatcher::OnLevelChanged().
  MemoryPressure current = current_memory_pressure_level_.load(std::memory_order_acquire);
  bool high_pressure = current > MemoryPressure::kLow ||
                       (current == MemoryPressure::kUnknown &&
                        superblock_info_->GetPageCount(CountType::kDirtyData) +
                                superblock_info_->GetPageCount(CountType::kDirtyDents) +
                                superblock_info_->GetPageCount(CountType::kDirtyNodes) >=
                            kMaxDirtyDataPages);

  switch (status) {
    case MemoryStatus::kNeedReclaim:
      return high_pressure;

    case MemoryStatus::kNeedCheckpoint:
      return high_pressure && (superblock_info_->GetPageCount(CountType::kDirtyDents) > 0 ||
                               superblock_info_->GetPageCount(CountType::kDirtyNodes) > 0);

    case MemoryStatus::kNeedWriteback:
      return high_pressure && superblock_info_->GetPageCount(CountType::kDirtyData) > 0;
  };

  return false;
}

void F2fs::WaitForAvailableMemory() {
  if (!GetMemoryStatus(MemoryStatus::kNeedReclaim)) {
    return;
  }
  fs::SharedLock lock(writeback_mutex_);
  memory_cvar_.wait_for(lock, std::chrono::seconds(kWriteTimeOut), [this]() {
    if (segment_manager_->HasNotEnoughFreeSecs(0, kDefaultBlocksPerSegment) ||
        (!GetMemoryStatus(MemoryStatus::kNeedWriteback) &&
         !GetMemoryStatus(MemoryStatus::kNeedCheckpoint))) {
      return true;
    }
    ScheduleWritebackAndReclaimPages();
    return false;
  });
}

// This function balances dirty node and dentry pages.
// In addition, it controls garbage collection.
void F2fs::BalanceFs(uint32_t needed) {
  if (IsOnRecovery()) {
    return;
  }

  WaitForAvailableMemory();
  if (segment_manager_->HasNotEnoughFreeSecs(0, needed)) {
    // Wait for writeback before gc.
    std::lock_guard lock(f2fs::GetGlobalLock());
    AllocateFreeSections(needed);
  }
}

void F2fs::AllocateFreeSections(uint32_t needed) {
  if (IsOnRecovery()) {
    return;
  }
  if (segment_manager_->HasNotEnoughFreeSecs(0, needed)) {
    if (auto ret = StartGc(needed); ret.is_error()) {
      // StartGc() returns ZX_ERR_UNAVAILABLE when there is no available victim section.
      ZX_DEBUG_ASSERT(ret.error_value() == ZX_ERR_UNAVAILABLE ||
                      superblock_info_->TestCpFlags(CpFlag::kCpErrorFlag));
    }
  }
}

zx::result<uint32_t> F2fs::StartGc(uint32_t needed) {
  // For testing
  if (!segment_manager_->CanGc()) {
    return zx::ok(0);
  }

  if (superblock_info_->TestCpFlags(CpFlag::kCpErrorFlag)) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  GcType gc_type = GcType::kBgGc;
  uint32_t before = segment_manager_->FreeSections(), prefree = 0;

  // If there are prefree_segments, do checkpoint to make them free.
  if (segment_manager_->PrefreeSegments()) {
    if (zx_status_t ret = WriteCheckpointUnsafe(false); ret != ZX_OK) {
      return zx::error(ret);
    }
  }
  if (segment_manager_->HasEnoughSsrBlocks(needed).is_ok()) {
    return zx::ok(segment_manager_->FreeSections() - before);
  }
  // During gc, f2fs::GetGlobalLock() protects filesystem from modification.
  while (segment_manager_->HasNotEnoughFreeSecs(prefree, needed)) {
    if (superblock_info_->TestCpFlags(CpFlag::kCpErrorFlag)) {
      return zx::error(ZX_ERR_BAD_STATE);
    }

    if (gc_type == GcType::kBgGc) {
      gc_type = GcType::kFgGc;
    }

    auto segno = segment_manager_->GetGcVictim(gc_type, CursegType::kNoCheckType);
    if (segno.is_error()) {
      break;
    }
    if (auto err = segment_manager_->DoGarbageCollect(*segno, gc_type); err != ZX_OK) {
      return zx::error(err);
    }
    ++prefree;
    if (gc_type == GcType::kFgGc) {
      segment_manager_->SetCurVictimSec(kNullSecNo);
      // Do checkpoint when there are too many dirty node/dir pages.
      if (segment_manager_->HasNotEnoughFreeSecs()) {
        if (zx_status_t ret = WriteCheckpointUnsafe(false); ret != ZX_OK) {
          return zx::error(ret);
        }
        prefree = 0;
      }
    }
  }
  uint32_t sec_freed = segment_manager_->FreeSections() - before + prefree;
  if (!sec_freed) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  num_gc_runs_ += sec_freed;
  return zx::ok(sec_freed);
}

}  // namespace f2fs
