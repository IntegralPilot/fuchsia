// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/vnode.h"

#include <sys/stat.h>
#include <zircon/syscalls-next.h>

#include <span>

#include <fbl/string_buffer.h>
#include <fbl/unique_fd.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/dir.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/segment.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vmo_manager.h"
#include "src/storage/f2fs/vnode_cache.h"
#include "src/storage/f2fs/writeback.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

VnodeF2fs::VnodeF2fs(F2fs* fs, ino_t ino, umode_t mode, LockedPage node_page)
    : PagedVnode(*fs->vfs()),
      superblock_info_(fs->GetSuperblockInfo()),
      ino_(ino),
      fs_(fs),
      mode_(mode) {
  SetFlag(InodeInfoFlag::kActive);
  if (IsMeta() || IsNode()) {
    CreateFileCacheUnsafe();
    return;
  }

  InitExtentTree();
  if (node_page) {
    InitializeFromPage(node_page);
    return;
  }

  SetFlag(InodeInfoFlag::kNewInode);
  nlink_ = 1;
  uid_ = getuid();

  timespec cur;
  clock_gettime(CLOCK_REALTIME, &cur);
  time_ = std::make_unique<Timestamps>(UpdateMode::kRelative, cur, cur, cur, cur);

  generation_ = superblock_info_.GetNextGeneration();
  superblock_info_.IncNextGeneration();
  if (superblock_info_.TestOpt(MountOption::kInlineXattr)) {
    SetFlag(InodeInfoFlag::kInlineXattr);
    inline_xattr_size_ = kInlineXattrAddrs;
  }

  if (superblock_info_.TestOpt(MountOption::kInlineDentry) && IsDir()) {
    SetFlag(InodeInfoFlag::kInlineDentry);
    SetInlineXattrSize(kInlineXattrAddrs);
  }
  CreateFileCacheUnsafe();
}

VnodeF2fs::~VnodeF2fs() { ReleasePagedVmo(); }

fuchsia_io::NodeProtocolKinds VnodeF2fs::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kFile;
}

void VnodeF2fs::SetMode(const umode_t& mode) { mode_ = mode; }

umode_t VnodeF2fs::GetMode() const { return mode_; }

bool VnodeF2fs::IsDir() const { return S_ISDIR(mode_); }

bool VnodeF2fs::IsReg() const { return S_ISREG(mode_); }

bool VnodeF2fs::IsLink() const { return S_ISLNK(mode_); }

bool VnodeF2fs::IsChr() const { return S_ISCHR(mode_); }

bool VnodeF2fs::IsBlk() const { return S_ISBLK(mode_); }

bool VnodeF2fs::IsSock() const { return S_ISSOCK(mode_); }

bool VnodeF2fs::IsFifo() const { return S_ISFIFO(mode_); }

bool VnodeF2fs::HasGid() const { return mode_ & S_ISGID; }

bool VnodeF2fs::IsNode() const { return ino_ == superblock_info_.GetNodeIno(); }

bool VnodeF2fs::IsMeta() const { return ino_ == superblock_info_.GetMetaIno(); }

zx_status_t VnodeF2fs::GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) {
  zx::result size = CreatePagedVmo(GetSize());
  if (size.is_error()) {
    return size.error_value();
  }
  return ClonePagedVmo(flags, *size, out_vmo);
}

zx::result<size_t> VnodeF2fs::CreatePagedVmo(size_t size) {
  if (!paged_vmo().is_valid()) {
    if (zx::result status = EnsureCreatePagedVmo(size, ZX_VMO_UNBOUNDED | ZX_VMO_TRAP_DIRTY);
        status.is_error()) {
      return status.take_error();
    }
    SetPagedVmoName();
  }
  return zx::ok(size);
}

void VnodeF2fs::SetPagedVmoName() {
  fbl::StringBuffer<ZX_MAX_NAME_LEN> name;
  name.Clear();
  name.AppendPrintf("%s-%.8s-%u", "f2fs", name_.data(), GetKey());
  paged_vmo().set_property(ZX_PROP_NAME, name.data(), name.size());
}

zx_status_t VnodeF2fs::ClonePagedVmo(fuchsia_io::wire::VmoFlags flags, size_t size,
                                     zx::vmo* out_vmo) {
  if (!paged_vmo()) {
    return ZX_ERR_NOT_FOUND;
  }

  zx_rights_t rights = ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY;
  rights |= (flags & fuchsia_io::wire::VmoFlags::kRead) ? ZX_RIGHT_READ : 0;
  rights |= (flags & fuchsia_io::wire::VmoFlags::kWrite) ? ZX_RIGHT_WRITE : 0;

  uint32_t options = 0;
  if (flags & fuchsia_io::wire::VmoFlags::kPrivateClone) {
    options = ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE;
    // Allowed only on private vmo.
    rights |= ZX_RIGHT_SET_PROPERTY;
  } else if (flags & fuchsia_io::wire::VmoFlags::kWrite) {
    // Shared writable VMOs use child slices rather than child references: a child slice
    // directly shares pager pages with the parent VMO for writable mmap mappings, but
    // maintains an independent StreamSizeManager. This prevents out-of-band stream size
    // modifications via zx_vmo_set_stream_size from affecting the parent VMO or bypassing
    // filesystem truncate locking and metadata consistency checks.
    options = ZX_VMO_CHILD_SLICE;
    size = fbl::round_up(size, zx_system_get_page_size());
  } else {
    // |size| should be 0 with ZX_VMO_CHILD_REFERENCE.
    size = 0;
    options = ZX_VMO_CHILD_REFERENCE;
  }

  if (!(flags & fuchsia_io::wire::VmoFlags::kWrite)) {
    options |= ZX_VMO_CHILD_NO_WRITE;
  }

  zx::vmo vmo;
  if (auto status = paged_vmo().create_child(options, 0, size, &vmo); status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to duplicate VMO: " << zx_status_get_string(status);
    return status;
  }
  DidClonePagedVmo();

  if (auto status = vmo.replace(rights, &vmo); status != ZX_OK) {
    return status;
  }
  *out_vmo = std::move(vmo);
  return ZX_OK;
}

void VnodeF2fs::VmoRead(uint64_t offset, uint64_t length) {
  zx::vmo vmo;
  zx::result size = CreateAndPopulateVmo(vmo, offset, length);
  if (unlikely(!paged_vmo())) {
    // Races with calling FreePagedVmo() on another thread can result in stale read requests. Ignore
    // them if the VMO is gone.
    FX_LOGS(WARNING) << "A pager-backed VMO is already freed: " << ZX_ERR_NOT_FOUND;
    return;
  }
  if (unlikely(size.is_error())) {
    return ReportPagerErrorUnsafe(ZX_PAGER_VMO_READ, offset, length, size.error_value());
  }
  std::optional vfs = this->vfs();
  ZX_DEBUG_ASSERT(vfs.has_value());
  if (auto ret = vfs.value().get().SupplyPages(paged_vmo(), offset, *size, std::move(vmo), 0);
      ret.is_error()) {
    ReportPagerErrorUnsafe(ZX_PAGER_VMO_READ, offset, length, ret.error_value());
  }
}

zx::result<size_t> VnodeF2fs::CreateAndPopulateVmo(zx::vmo& vmo, const size_t offset,
                                                   const size_t length) {
  const size_t file_size = GetSize();
  const size_t max_bytes = fbl::round_up(file_size, kBlockSize);
  const size_t max_block = max_bytes / kBlockSize;
  const size_t start_block = offset / kBlockSize;
  if (start_block >= max_block) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  size_t end_offset;
  if (!safemath::CheckAdd(offset, length).AssignIfValid(&end_offset) || length == 0) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  const size_t end_block =
      end_offset >= max_bytes ? max_block : fbl::round_up(end_offset, kBlockSize) / kBlockSize;
  ZX_DEBUG_ASSERT(start_block < end_block);
  const size_t request_blocks = end_block - start_block;
  size_t num_read_blocks = 0;

  // Do not readahead if it has inline data or memory pressure is high.
  if (!TestFlag(InodeInfoFlag::kInlineData) && !TestFlag(InodeInfoFlag::kNoAlloc) &&
      offset < file_size) {
    size_t num_readahead_blocks = std::min(max_block, end_block + kMaxReadaheadSize) - start_block;
    num_read_blocks = file_cache_->GetReadHint(start_block, request_blocks, num_readahead_blocks,
                                               fs()->GetMemoryStatus(MemoryStatus::kNeedReclaim));
  }

  zx::result addrs = FindAddresses(start_block, num_read_blocks);
  if (addrs.is_error()) {
    return addrs.take_error();
  }

  // Read blocks only for valid block addrs.
  num_read_blocks = 0;
  uint32_t num_checked_addrs = 0;
  for (auto& addr : *addrs) {
    ++num_checked_addrs;
    if (addr != kNewAddr && addr != kNullAddr) {
      num_read_blocks = num_checked_addrs;
    }
    if (num_checked_addrs == request_blocks && !num_read_blocks) {
      // We can skip disk I/Os as well as readahead.
      break;
    }
  }

  // Create vmo to feed paged vmo.
  size_t vmo_size = std::max(request_blocks, num_read_blocks) * kBlockSize;
  if (auto status = zx::vmo::create(vmo_size, 0, &vmo); status != ZX_OK) {
    return zx::error(status);
  }

  if (num_read_blocks) {
    addrs->resize(num_read_blocks, kNullAddr);
    if (auto status = fs()->MakeReadOperations(vmo, *addrs, PageType::kData); status.is_error()) {
      return status.take_error();
    }
    // Load read pages on FileCache as hints of readahead. It's okay to fail because the failure
    // doesn't affect read operations but only readahead.
    [[maybe_unused]] zx::result ret = GrabPages(start_block, start_block + num_read_blocks);
  }
  return zx::ok(vmo_size);
}

void VnodeF2fs::VmoDirty(uint64_t offset, uint64_t length) {
  std::optional vfs = this->vfs();
  ZX_DEBUG_ASSERT(vfs.has_value());
  auto ret = vfs.value().get().DirtyPages(paged_vmo(), offset, length);
  if (ret.is_error()) {
    // If someone has already dirtied or truncated these pages, do nothing.
    if (ret.error_value() == ZX_ERR_NOT_FOUND) {
      return;
    }
    ReportPagerErrorUnsafe(ZX_PAGER_OP_DIRTY, offset, length, ret.error_value());
  }
}

void VnodeF2fs::OnNoPagedVmoClones() {
  // Override PagedVnode::OnNoPagedVmoClones().
  // We intend to keep PagedVnode::paged_vmo alive while this vnode has any reference.
  ZX_DEBUG_ASSERT(!has_clones());
}

void VnodeF2fs::ReportPagerError(const uint32_t op, const uint64_t offset, const uint64_t length,
                                 const zx_status_t err) {
  fs::SharedLock lock(mutex_);
  return ReportPagerErrorUnsafe(op, offset, length, err);
}

void VnodeF2fs::ReportPagerErrorUnsafe(const uint32_t op, const uint64_t offset,
                                       const uint64_t length, const zx_status_t err) {
  std::optional vfs = this->vfs();
  ZX_DEBUG_ASSERT(vfs.has_value());
  zx_status_t pager_err = err;
  // Notifies the kernel that a page request for the given `range` has failed. Sent in response
  // to a `ZX_PAGER_VMO_READ` or `ZX_PAGER_VMO_DIRTY` page request. See `ZX_PAGER_OP_FAIL` for
  // more information.
  if (err != ZX_ERR_IO && err != ZX_ERR_IO_DATA_INTEGRITY && err != ZX_ERR_BAD_STATE &&
      err != ZX_ERR_NO_SPACE && err != ZX_ERR_BUFFER_TOO_SMALL) {
    pager_err = ZX_ERR_BAD_STATE;
  }
  FX_LOGS(WARNING) << "Failed to handle a pager request(" << std::hex << op << "). "
                   << zx_status_get_string(err);
  if (auto result = vfs.value().get().ReportPagerError(paged_vmo(), offset, length, pager_err);
      result.is_error()) {
    FX_LOGS(ERROR) << "Failed to report a pager error. " << result.status_string();
  }
}

void VnodeF2fs::RecycleNode() {
  do {
    std::lock_guard lock(mutex_);
    if (open_count()) {
      FX_LOGS(WARNING) << "RecycleNode[" << GetNameViewUnsafe().data() << ":" << GetKey()
                       << "]: open_count must be zero (" << open_count() << ")";
    }
    // It is safe to free vnodes that have been already evicted from vnode cache.
    if (!(*this).fbl::WAVLTreeContainable<VnodeF2fs*>::InContainer()) {
      break;
    }
    if (GetLinkCountUnsafe()) {
      // This should not happen, as F2FS releases the last references to dirty vnodes after
      // writeback during checkpoint, while disallowing any file operations that could generate
      // additional dirty pages.
      if (GetDirtyPageCount()) {
        FX_LOGS(WARNING) << "Vnode[" << GetNameViewUnsafe().data() << ":" << GetKey()
                         << "] is deleted with " << GetDirtyPageCount() << " of dirty pages"
                         << ". CpFlag::kCpErrorFlag is "
                         << (superblock_info_.TestCpFlags(CpFlag::kCpErrorFlag) ? "set."
                                                                                : "not set.");
      }
      if (fs()->GetMemoryStatus(MemoryStatus::kNeedReclaim)) {
        CleanupCache();
      }
      PutRefPtr();
      return;
    }
    PutRefPtr();
    ZX_ASSERT_MSG(
        false,
        "An orphan(%s, 0x%x) should not drop to a zero refcount while it is preserved in the vnode cache",
        GetNameViewUnsafe().data(), GetKey());
    return;
  } while (false);
  delete this;
}

zx::result<fs::VnodeAttributes> VnodeF2fs::GetAttributes() const {
  fs::VnodeAttributes a;
  fs::SharedLock rlock(mutex_);
  a.mode = mode_;
  a.id = ino_;
  a.content_size = vmo_manager().GetContentSize();
  a.storage_size = GetBlockCountUnsafe() * kBlockSize;
  a.link_count = GetLinkCountUnsafe();
  const timespec& atime = GetTime<Timestamps::AccessTime>();
  const timespec& btime = GetTime<Timestamps::BirthTime>();
  const timespec& mtime = GetTime<Timestamps::ModificationTime>();
  const timespec& ctime = GetTime<Timestamps::ChangeTime>();
  a.creation_time = zx_time_add_duration(ZX_SEC(btime.tv_sec), btime.tv_nsec);
  a.modification_time = zx_time_add_duration(ZX_SEC(mtime.tv_sec), mtime.tv_nsec);
  a.change_time = zx_time_add_duration(ZX_SEC(ctime.tv_sec), ctime.tv_nsec);
  a.access_time = zx_time_add_duration(ZX_SEC(atime.tv_sec), atime.tv_nsec);

  return zx::ok(a);
}

fs::VnodeAttributesQuery VnodeF2fs::SupportedMutableAttributes() const {
  return fs::VnodeAttributesQuery::kCreationTime | fs::VnodeAttributesQuery::kModificationTime;
}

zx::result<> VnodeF2fs::UpdateAttributes(const fs::VnodeAttributesUpdate& attr) {
  bool need_inode_sync = false;
  std::lock_guard file_lock(mutex_);
  if (attr.creation_time) {
    SetTime<Timestamps::BirthTime>(
        zx_timespec_from_duration(safemath::checked_cast<zx_duration_t>(*attr.creation_time)));
    need_inode_sync = true;
  }
  if (attr.modification_time) {
    SetTime<Timestamps::ModificationTime>(
        zx_timespec_from_duration(safemath::checked_cast<zx_duration_t>(*attr.modification_time)));
    need_inode_sync = true;
  }

  if (need_inode_sync) {
    SetDirty();
  }

  return zx::ok();
}

struct f2fs_iget_args {
  uint64_t ino;
  int on_free;
};

#if 0  // porting needed
// void VnodeF2fs::F2fsSetInodeFlags() {
  // uint64_t &flags = fi.i_flags;

  // inode_.i_flags &= ~(S_SYNC | S_APPEND | S_IMMUTABLE |
  //     S_NOATIME | S_DIRSYNC);

  // if (flags & FS_SYNC_FL)
  //   inode_.i_flags |= S_SYNC;
  // if (flags & FS_APPEND_FL)
  //   inode_.i_flags |= S_APPEND;
  // if (flags & FS_IMMUTABLE_FL)
  //   inode_.i_flags |= S_IMMUTABLE;
  // if (flags & FS_NOATIME_FL)
  //   inode_.i_flags |= S_NOATIME;
  // if (flags & FS_DIRSYNC_FL)
  //   inode_.i_flags |= S_DIRSYNC;
// }

// int VnodeF2fs::F2fsIgetTest(void *data) {
  // f2fs_iget_args *args = (f2fs_iget_args *)data;

  // if (ino_ != args->ino)
  //   return 0;
  // if (i_state & (I_FREEING | I_WILL_FREE)) {
  //   args->on_free = 1;
  //   return 0;
  // }
  // return 1;
// }

// VnodeF2fs *VnodeF2fs::F2fsIgetNowait(uint64_t ino) {
//   fbl::RefPtr<VnodeF2fs> vnode_refptr;
//   VnodeF2fs *vnode = nullptr;
//   f2fs_iget_args args = {.ino = ino, .on_free = 0};
//   vnode = ilookup5(sb, ino, F2fsIgetTest, &args);

//   if (vnode)
//     return vnode;
//   if (!args.on_free) {
//     fs()->GetVnode(ino, &vnode_refptr);
//     vnode = vnode_refptr.get();
//     return vnode;
//   }
//   return static_cast<VnodeF2fs *>(ErrPtr(ZX_ERR_NOT_FOUND));
// }
#endif

void VnodeF2fs::UpdateInodePage(LockedPage& inode_page, bool update_size) {
  fs::SharedLock lock(mutex_);
  UpdateInodePageUnsafe(inode_page, update_size);
}

void VnodeF2fs::UpdateInodePageUnsafe(LockedPage& inode_page, bool update_size) {
  inode_page.WaitOnWriteback();
  Inode& inode = inode_page->GetAddress<Node>()->i;
  uint64_t content_size = GetSize(update_size);
  if (update_size) {
    ClearFlag(InodeInfoFlag::kSyncInode);
  }
  inode.i_size = CpuToLe(content_size);
  inode.i_mode = CpuToLe(GetMode());
  inode.i_advise = advise_;
  inode.i_uid = CpuToLe(uid_);
  inode.i_gid = CpuToLe(gid_);
  inode.i_links = CpuToLe(GetLinkCountUnsafe());
  // For on-disk i_blocks, we keep counting inode block for backward compatibility.
  inode.i_blocks = CpuToLe(safemath::CheckAdd<uint64_t>(GetBlockCountUnsafe(), 1).ValueOrDie());

  if (ExtentCacheAvailable()) {
    auto extent_info = extent_tree_->GetLargestExtent();
    inode.i_ext.blk_addr = CpuToLe(extent_info.blk_addr);
    inode.i_ext.fofs = CpuToLe(static_cast<uint32_t>(extent_info.fofs));
    inode.i_ext.len = CpuToLe(extent_info.len);
  } else {
    std::memset(&inode.i_ext, 0, sizeof(inode.i_ext));
  }

  // TODO(b/297201368): As there is no space for creation time, it temporarily considers ctime as
  // creation time.
  const timespec& atime = time_->Get<Timestamps::AccessTime>();
  const timespec& ctime = time_->Get<Timestamps::BirthTime>();
  const timespec& mtime = time_->Get<Timestamps::ModificationTime>();

  inode.i_atime = CpuToLe(static_cast<uint64_t>(atime.tv_sec));
  inode.i_ctime = CpuToLe(static_cast<uint64_t>(ctime.tv_sec));
  inode.i_mtime = CpuToLe(static_cast<uint64_t>(mtime.tv_sec));
  inode.i_atime_nsec = CpuToLe(static_cast<uint32_t>(atime.tv_nsec));
  inode.i_ctime_nsec = CpuToLe(static_cast<uint32_t>(ctime.tv_nsec));
  inode.i_mtime_nsec = CpuToLe(static_cast<uint32_t>(mtime.tv_nsec));
  inode.i_current_depth = CpuToLe(static_cast<uint32_t>(current_depth_));
  inode.i_xattr_nid = CpuToLe(xattr_nid_);
  inode.i_flags = CpuToLe(inode_flags_);
  inode.i_pino = CpuToLe(GetParentNid());
  inode.i_generation = CpuToLe(generation_);
  inode.i_dir_level = dir_level_;

  std::string_view name(name_);
  // double check |name|
  ZX_DEBUG_ASSERT(IsValidNameLength(name));
  auto size = safemath::checked_cast<uint32_t>(name.size());
  inode.i_namelen = CpuToLe(size);
  name.copy(reinterpret_cast<char*>(&inode.i_name[0]), size);

  if (TestFlag(InodeInfoFlag::kInlineData)) {
    inode.i_inline |= kInlineData;
  } else {
    inode.i_inline &= ~kInlineData;
  }
  if (TestFlag(InodeInfoFlag::kInlineDentry)) {
    inode.i_inline |= kInlineDentry;
  } else {
    inode.i_inline &= ~kInlineDentry;
  }
  if (extra_isize_) {
    inode.i_inline |= kExtraAttr;
    inode.i_extra_isize = extra_isize_;
    if (TestFlag(InodeInfoFlag::kInlineXattr)) {
      inode.i_inline_xattr_size = CpuToLe(inline_xattr_size_);
    }
  }
  if (TestFlag(InodeInfoFlag::kDataExist)) {
    inode.i_inline |= kDataExist;
  } else {
    inode.i_inline &= ~kDataExist;
  }
  if (TestFlag(InodeInfoFlag::kInlineXattr)) {
    inode.i_inline |= kInlineXattr;
  } else {
    inode.i_inline &= ~kInlineXattr;
  }

  inode_page.SetDirty();
}

zx_status_t VnodeF2fs::DoTruncate(size_t len) {
  if (zx_status_t ret = TruncateBlocks(len); ret != ZX_OK) {
    return ret;
  }
  SetTime<Timestamps::ModificationTime>();
  if (!len) {
    ClearFlag(InodeInfoFlag::kDataExist);
  }

  SetDirty();
  return ZX_OK;
}

zx_status_t VnodeF2fs::TruncateBlocks(uint64_t from) {
  uint32_t blocksize = superblock_info_.GetBlocksize();
  if (from > GetSize()) {
    return ZX_OK;
  }

  pgoff_t free_from =
      safemath::CheckRsh(fbl::round_up(from, blocksize), superblock_info_.GetLogBlocksize())
          .ValueOrDie();
  // Invalidate data pages starting from |free_from|, and purge the addrs of invalidated pages from
  // nodes.
  InvalidatePages(free_from);
  return InvalidateBlocks(free_from);
}

zx_status_t VnodeF2fs::TruncateHole(pgoff_t pg_start, pgoff_t pg_end, bool evict) {
  std::lock_guard lock(mutex_);
  return TruncateHoleUnsafe(pg_start, pg_end, evict);
}

zx_status_t VnodeF2fs::TruncateHoleUnsafe(pgoff_t pg_start, pgoff_t pg_end, bool evict) {
  std::vector<LockedPage> pages;
  if (evict) {
    pages = InvalidatePages(pg_start, pg_end);
  }
  for (pgoff_t index = pg_start; index < pg_end; ++index) {
    zx::result path = GetNodePath(index);
    if (path.is_error()) {
      if (path.error_value() == ZX_ERR_NOT_FOUND) {
        continue;
      }
      return path.error_value();
    }
    zx::result dnode_page = fs()->GetNodeManager().GetLockedDnodePage(*path, IsDir());
    if (dnode_page.is_error()) {
      if (dnode_page.error_value() == ZX_ERR_NOT_FOUND) {
        continue;
      }
      return dnode_page.error_value();
    }
    AddBlocksUnsafe(path->num_new_nodes);
    size_t ofs_in_dnode = GetOfsInDnode(*path);
    NodePage& node = (*dnode_page).GetPage<NodePage>();
    if (node.GetBlockAddr(ofs_in_dnode) != kNullAddr) {
      TruncateDnodeAddrs(*dnode_page, ofs_in_dnode, 1);
    }
  }
  return ZX_OK;
}

void VnodeF2fs::ReleasePagedVmo() {
  if (paged_vmo()) {
    fbl::RefPtr<fs::Vnode> pager_reference = FreePagedVmo();
    ZX_DEBUG_ASSERT(!pager_reference);
  }
}

void VnodeF2fs::PurgeDataBlocks() {
  if (ino_ == superblock_info_.GetNodeIno() || ino_ == superblock_info_.GetMetaIno()) {
    return;
  }

  if (GetLinkCountUnsafe()) {
    return;
  }

  block_t base = xattr_nid_ ? 1 : 0;
  if (GetBlockCountUnsafe() > base && (IsDir() || IsReg() || IsLink())) {
    InvalidatePages(0);
    InvalidateBlocks(0);
  }
}

zx_status_t VnodeF2fs::InvalidateBlocks(size_t from) {
  {
    zx::result path = GetNodePath(from);
    if (path.is_error()) {
      return path.error_value();
    }
    zx::result node_page = fs()->GetNodeManager().FindLockedDnodePage(*path);
    if (node_page.is_ok()) {
      size_t ofs_in_node = GetOfsInDnode(*path);
      // If |from| starts from inode or the middle of dnode, purge the addrs in the start dnode.
      NodePage& node = (*node_page).GetPage<NodePage>();
      if (ofs_in_node || node.IsInode()) {
        size_t count = 0;
        if (node.IsInode()) {
          count = safemath::CheckSub(GetAddrsPerInode(), ofs_in_node).ValueOrDie();
        } else {
          count = safemath::CheckSub(kAddrsPerBlock, ofs_in_node).ValueOrDie();
        }
        TruncateDnodeAddrs(*node_page, ofs_in_node, count);
        from += count;
      }
    } else if (node_page.error_value() != ZX_ERR_NOT_FOUND) {
      return node_page.error_value();
    }
  }
  // Invalidate the rest nodes.
  if (zx_status_t err = TruncateInodeBlocks(from); err != ZX_OK) {
    return err;
  }
  return ZX_OK;
}

zx_status_t VnodeF2fs::CreateFileCache(uint64_t nbytes) {
  std::lock_guard lock(mutex_);
  return CreateFileCacheUnsafe(nbytes);
}

zx_status_t VnodeF2fs::CreateFileCacheUnsafe(uint64_t nbytes) {
  zx::vmo vmo;
  VmoMode mode;
  size_t vmo_node_size = 0;

  if (file_cache_) {
    return ZX_ERR_ALREADY_EXISTS;
  }
  if (IsReg()) {
    if (zx::result size = CreatePagedVmo(nbytes); size.is_ok()) {
      zx_rights_t right =
          ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHTS_PROPERTY | ZX_RIGHT_READ | ZX_RIGHT_WRITE;
      ZX_ASSERT(paged_vmo().duplicate(right, &vmo) == ZX_OK);
      mode = VmoMode::kPaged;
      vmo_node_size = zx_system_get_page_size();
    }
  } else {
    mode = VmoMode::kDiscardable;
    vmo_node_size = kVmoNodeSize;
  }
  ZX_ASSERT(!(zx_system_get_page_size() % kBlockSize));
  ZX_ASSERT(!(vmo_node_size % zx_system_get_page_size()));
  vmo_manager_ = std::make_unique<VmoManager>(mode, nbytes, vmo_node_size, std::move(vmo));
  file_cache_ = std::make_unique<FileCache>(this, vmo_manager_.get());
  return ZX_OK;
}

void VnodeF2fs::InitializeFromPage(LockedPage& node_page) {
  Inode& inode = node_page->GetAddress<Node>()->i;
  std::string_view name(reinterpret_cast<char*>(inode.i_name),
                        std::min(kMaxNameLen, inode.i_namelen));

  name_ = name;
  uid_ = LeToCpu(inode.i_uid);
  gid_ = LeToCpu(inode.i_gid);
  nlink_ = LeToCpu(inode.i_links);
  // Don't count the in-memory inode.i_blocks for compatibility with the generic
  // filesystem including linux f2fs.
  num_blocks_ =
      safemath::CheckSub(safemath::checked_cast<block_t>(LeToCpu(inode.i_blocks)), 1).ValueOrDie();
  const timespec atime = {static_cast<time_t>(LeToCpu(inode.i_atime)),
                          static_cast<time_t>(LeToCpu(inode.i_atime_nsec))};
  // TODO(b/297201368): As there is no space for creation time, it temporarily considers ctime as
  // creation time.
  const timespec btime = {static_cast<time_t>(LeToCpu(inode.i_ctime)),
                          static_cast<time_t>(LeToCpu(inode.i_ctime_nsec))};
  const timespec mtime = {static_cast<time_t>(LeToCpu(inode.i_mtime)),
                          static_cast<time_t>(LeToCpu(inode.i_mtime_nsec))};
  time_ = std::make_unique<Timestamps>(UpdateMode::kRelative, atime, btime, mtime, mtime);
  generation_ = LeToCpu(inode.i_generation);
  SetParentNid(LeToCpu(inode.i_pino));
  current_depth_ = LeToCpu(inode.i_current_depth);
  xattr_nid_ = LeToCpu(inode.i_xattr_nid);
  inode_flags_ = LeToCpu(inode.i_flags);
  dir_level_ = inode.i_dir_level;
  data_version_ = superblock_info_.GetCheckpointVer() - 1;
  advise_ = inode.i_advise;

  if (inode.i_inline & kInlineDentry) {
    SetFlag(InodeInfoFlag::kInlineDentry);
    inline_xattr_size_ = kInlineXattrAddrs;
  }
  if (inode.i_inline & kInlineData) {
    SetFlag(InodeInfoFlag::kInlineData);
  }
  if (inode.i_inline & kInlineXattr) {
    SetFlag(InodeInfoFlag::kInlineXattr);
    inline_xattr_size_ = kInlineXattrAddrs;
  }
  if (inode.i_inline & kExtraAttr) {
    extra_isize_ = LeToCpu(inode.i_extra_isize);
    if (inode.i_inline & kInlineXattr) {
      inline_xattr_size_ = LeToCpu(inode.i_inline_xattr_size);
    }
  }
  if (inode.i_inline & kDataExist) {
    SetFlag(InodeInfoFlag::kDataExist);
  }
  if (extent_tree_ && inode.i_ext.blk_addr) {
    auto extent_info = ExtentInfo{.fofs = LeToCpu(inode.i_ext.fofs),
                                  .blk_addr = LeToCpu(inode.i_ext.blk_addr),
                                  .len = LeToCpu(inode.i_ext.len)};
    if (auto result = extent_tree_->InsertExtent(extent_info); result.is_error()) {
      SetFlag(InodeInfoFlag::kNoExtent);
    }
  }

  // Only orphan inodes may create file caches during recovery
  if (!fs()->IsOnRecovery() || !GetLinkCountUnsafe()) {
    CreateFileCacheUnsafe(LeToCpu(inode.i_size));
  }
}

bool VnodeF2fs::SetDirty() {
  if (IsNode() || IsMeta() || TestFlag(InodeInfoFlag::kNoAlloc)) {
    return false;
  }
  return fs()->GetVCache().AddDirty(*this) == ZX_OK;
}

bool VnodeF2fs::ClearDirty() {
  if (GetDirtyPageCount()) {
    file_cache_->ClearDirtyPages();
  }
  return fs()->GetVCache().RemoveDirty(this) == ZX_OK;
}

bool VnodeF2fs::IsDirty() { return fs()->GetVCache().IsDirty(*this); }

void VnodeF2fs::Sync(SyncCallback closure) { closure(SyncFile()); }

bool VnodeF2fs::NeedToCheckpoint() {
  if (!IsReg()) {
    return true;
  }
  if (GetLinkCount() != 1) {
    return true;
  }
  if (TestFlag(InodeInfoFlag::kNeedCp)) {
    return true;
  }
  if (!superblock_info_.SpaceForRollForward()) {
    return true;
  }
  if (NeedToSyncDir()) {
    return true;
  }
  if (superblock_info_.TestOpt(MountOption::kDisableRollForward)) {
    return true;
  }
  if (fs()->FindVnodeSet(VnodeSet::kModifiedDir, GetParentNid())) {
    return true;
  }
  return false;
}

void VnodeF2fs::SetSize(const size_t nbytes) {
  ZX_ASSERT(vmo_manager_);
  vmo_manager().SetContentSize(nbytes);
}

uint64_t VnodeF2fs::GetSize(bool update_checkpointed_size) const {
  ZX_ASSERT(vmo_manager_);
  return vmo_manager().GetContentSize(update_checkpointed_size);
}

bool VnodeF2fs::NeedToLogInode() const {
  return TestFlag(InodeInfoFlag::kSyncInode) || vmo_manager().SizeIsChanged();
}

zx_status_t VnodeF2fs::SyncFile(bool datasync) {
  if (superblock_info_.TestCpFlags(CpFlag::kCpErrorFlag)) {
    return ZX_ERR_BAD_STATE;
  }

  if (!IsDirty()) {
    return ZX_OK;
  }
  if (fs_->GetSegmentManager().HasNotEnoughFreeSecs(0, GetDirtyPageCount()) || NeedToCheckpoint()) {
    // Disallow filesystem modifications
    std::lock_guard lock(f2fs::GetGlobalLock());
    do {
      uint32_t to_write = std::min(kDefaultBlocksPerSegment, GetDirtyPageCount());
      fs()->AllocateFreeSections(to_write);
      WritebackOperation op = {.to_write = to_write};
      Writeback(op);
    } while (GetDirtyPageCount());
    zx_status_t ret = fs()->WriteCheckpointUnsafe(false);
    if (ret == ZX_OK) {
      ClearFlag(InodeInfoFlag::kNeedCp);
    }
    return ret;
  }
  fs::SharedLock lock(f2fs::GetGlobalLock());
  // Disallow modifications to this file
  std::lock_guard file_lock(mutex_);
  WritebackOperation op;
  Writeback(op);
  if (!datasync || NeedToLogInode()) {
    LockedPage page;
    if (zx_status_t ret = fs()->GetNodeManager().GetNodePage(ino_, &page); ret != ZX_OK) {
      return ret;
    }
    UpdateInodePageUnsafe(page, true);
  }
  fs()->GetNodeManager().FsyncNodePages(Ino());
  if (!GetDirtyPageCount()) {
    ClearDirty();
  }
  return ZX_OK;
}

bool VnodeF2fs::NeedToSyncDir() const {
  ZX_DEBUG_ASSERT(GetParentNid() < kNullIno);
  return !fs()->GetNodeManager().IsCheckpointedNode(GetParentNid());
}

void VnodeF2fs::Notify(std::string_view name, fuchsia_io::wire::WatchEvent event) {
  watcher_.Notify(name, event);
}

zx_status_t VnodeF2fs::WatchDir(fs::FuchsiaVfs* vfs, fuchsia_io::wire::WatchMask mask,
                                uint32_t options,
                                fidl::ServerEnd<fuchsia_io::DirectoryWatcher> watcher) {
  return watcher_.WatchDir(vfs, this, mask, options, std::move(watcher));
}

bool VnodeF2fs::ExtentCacheAvailable() {
  return superblock_info_.TestOpt(MountOption::kReadExtentCache) && IsReg() &&
         !TestFlag(InodeInfoFlag::kNoExtent);
}

void VnodeF2fs::InitExtentTree() {
  if (!ExtentCacheAvailable()) {
    return;
  }

  // Because the lifecycle of an extent_tree is tied to the lifecycle of a vnode, the extent tree
  // should not exist when the vnode is created.
  ZX_DEBUG_ASSERT(!extent_tree_);
  extent_tree_ = std::make_unique<ExtentTree>();
}

zx::result<fbl::RefPtr<VnodeF2fs>> VnodeF2fs::GetRefPtr() {
  std::lock_guard lock(ref_mutex_);
  if (!TestFlag(InodeInfoFlag::kActive)) {
    fbl::RefPtr<VnodeF2fs> vnode = fbl::ImportFromRawPtr(this);
    SetFlag(InodeInfoFlag::kActive);
    return zx::ok(std::move(vnode));
  }

  fbl::RefPtr<VnodeF2fs> vnode = fbl::MakeRefPtrUpgradeFromRaw(this, ref_mutex_);
  if (!vnode) {
    flag_cvar_.wait(ref_mutex_, [this]() { return !TestFlag(InodeInfoFlag::kActive); });
    return zx::error(ZX_ERR_SHOULD_WAIT);
  }
  SetFlag(InodeInfoFlag::kActive);
  return zx::ok(std::move(vnode));
}

void VnodeF2fs::PutRefPtr() {
  std::lock_guard lock(ref_mutex_);
  ZX_ASSERT(IsActive());
  ResurrectRef();
  fbl::RefPtr<VnodeF2fs> vnode = fbl::ImportFromRawPtr(this);
  [[maybe_unused]] auto leak = fbl::ExportToRawPtr(&vnode);
  ClearFlag(InodeInfoFlag::kActive);
  flag_cvar_.notify_all();
}

bool VnodeF2fs::IsActive() const { return TestFlag(InodeInfoFlag::kActive); }

zx::result<PageBitmap> VnodeF2fs::GetBitmap(fbl::RefPtr<Page> page) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

void VnodeF2fs::SetOrphan() {
  std::lock_guard file_lock(mutex_);
  if (!SetFlag(InodeInfoFlag::kOrphan)) {
    if (fs()->IsOnRecovery()) {
      // No other operations are required during recovery.
      return;
    }
    fs()->AddToVnodeSet(VnodeSet::kOrphan, GetKey());
    if (IsDir()) {
      Notify(".", fuchsia_io::wire::WatchEvent::kDeleted);
    }
    // If no fd is open for |this|, it is safe to mark it clean and keep it frozen
    // until RecycleNode().
    if (open_count()) {
      return;
    }
    PurgeUnsafe();
  }
}

void VnodeF2fs::TruncateNode(LockedPage& page) {
  nid_t nid = static_cast<nid_t>(page->GetKey());
  fs_->GetNodeManager().TruncateNode(nid);
  if (nid == Ino()) {
    fs_->RemoveFromVnodeSet(VnodeSet::kOrphan, nid);
    superblock_info_.DecValidInodeCount();
  } else {
    RemoveBlocksUnsafe(1);
    SetDirty();
  }
  page.WaitOnWriteback();
  page.Invalidate();
  superblock_info_.SetDirty();
}

block_t VnodeF2fs::TruncateDnodeAddrs(LockedPage& dnode, size_t offset, size_t count) {
  block_t nr_free = 0;
  NodePage& node = dnode.GetPage<NodePage>();
  for (; count > 0; --count, ++offset) {
    block_t blkaddr = node.GetBlockAddr(offset);
    if (blkaddr == kNullAddr) {
      continue;
    }
    dnode.WaitOnWriteback();
    node.SetDataBlkaddr(offset, kNullAddr);
    UpdateExtentCache(node.StartBidxOfNode(GetAddrsPerInode()) + offset, kNullAddr);
    ++nr_free;
    if (blkaddr != kNewAddr) {
      fs()->GetSegmentManager().InvalidateBlocks(blkaddr);
    }
  }
  if (nr_free) {
    fs()->GetSuperblockInfo().DecValidBlockCount(nr_free);
    RemoveBlocksUnsafe(nr_free);
    dnode.SetDirty();
    SetDirty();
  }
  return nr_free;
}

zx::result<size_t> VnodeF2fs::TruncateDnode(nid_t nid) {
  if (!nid) {
    return zx::ok(1);
  }

  LockedPage page;
  // get direct node
  if (zx_status_t err = fs_->GetNodeManager().GetNodePage(nid, &page); err != ZX_OK) {
    // It is already invalid.
    if (err == ZX_ERR_NOT_FOUND) {
      return zx::ok(1);
    }
    return zx::error(err);
  }
  // If the node type is invalid (e.g. an Inode or IndirectNode where a DirectNode is expected),
  // treat it as already removed so the parent NID pointer is cleanly zeroed out.
  if (page.GetPage<NodePage>().IsInode() || !page.GetPage<NodePage>().IsDnode()) {
    return zx::ok(1);
  }

  TruncateDnodeAddrs(page, 0, kAddrsPerBlock);
  TruncateNode(page);
  return zx::ok(1);
}

zx::result<size_t> VnodeF2fs::TruncateNodes(nid_t start_nid, size_t nofs, size_t ofs,
                                            size_t depth) {
  ZX_DEBUG_ASSERT(depth == 2 || depth == 3);
  if (unlikely(depth < 2 || depth > 3)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  constexpr size_t kInvalidatedNids = kNidsPerBlock + 1;
  if (start_nid == 0) {
    return zx::ok(kInvalidatedNids);
  }

  LockedPage page;
  if (zx_status_t ret = fs_->GetNodeManager().GetNodePage(start_nid, &page); ret != ZX_OK) {
    if (ret != ZX_ERR_NOT_FOUND) {
      return zx::error(ret);
    }
    if (depth == 2) {
      return zx::ok(kInvalidatedNids);
    }
    return zx::ok(kInvalidatedNids * kNidsPerBlock + 1);
  }
  // If the node type is invalid (e.g. an Inode or DirectNode where an IndirectNode is expected),
  // treat the entire indirect subtree as already removed.
  if (page.GetPage<NodePage>().IsInode() || page.GetPage<NodePage>().IsDnode()) {
    return zx::ok(depth == 2 ? kInvalidatedNids : kInvalidatedNids * kNidsPerBlock + 1);
  }

  size_t child_nofs = 0, freed = 0;
  nid_t child_nid;
  IndirectNode& indirect_node = page->GetAddress<Node>()->in;
  if (depth < 3) {
    for (size_t i = ofs; i < kNidsPerBlock; ++i, ++freed) {
      child_nid = LeToCpu(indirect_node.nid[i]);
      if (child_nid == 0) {
        continue;
      }
      if (auto ret = TruncateDnode(child_nid); ret.is_error()) {
        return ret;
      }
      ZX_ASSERT(!page.GetPage<NodePage>().IsInode());
      page.WaitOnWriteback();
      page.GetPage<NodePage>().SetNid(i, 0);
      page.SetDirty();
    }
  } else {
    child_nofs = nofs + ofs * kInvalidatedNids + 1;
    for (size_t i = ofs; i < kNidsPerBlock; ++i) {
      child_nid = LeToCpu(indirect_node.nid[i]);
      zx::result num_freed_nodes = TruncateNodes(child_nid, child_nofs, 0, depth - 1);
      if (num_freed_nodes.is_error()) {
        return num_freed_nodes.take_error();
      }
      ZX_DEBUG_ASSERT(*num_freed_nodes == kInvalidatedNids);
      ZX_DEBUG_ASSERT(!page.GetPage<NodePage>().IsInode());
      page.WaitOnWriteback();
      page.GetPage<NodePage>().SetNid(i, 0);
      page.SetDirty();
      child_nofs += kInvalidatedNids;
      freed += kInvalidatedNids;
    }
  }

  if (!ofs) {
    TruncateNode(page);
    ++freed;
  }
  return zx::ok(freed);
}

zx_status_t VnodeF2fs::TruncatePartialNodes(const Inode& inode, const size_t (&offset)[4],
                                            size_t depth) {
  LockedPage pages[2];
  nid_t nid[3];
  size_t idx = depth - 2;

  if (nid[0] = LeToCpu(inode.i_nid[offset[0] - kNodeDir1Block]); !nid[0]) {
    return ZX_OK;
  }

  // get indirect nodes in the path
  for (size_t i = 0; i < idx + 1; ++i) {
    if (auto ret = fs_->GetNodeManager().GetNodePage(nid[i], &pages[i]); ret != ZX_OK) {
      return ret;
    }
    // Verify intermediate nodes in the path are IndirectNodes.
    if (pages[i].GetPage<NodePage>().IsInode() || pages[i].GetPage<NodePage>().IsDnode()) {
      return ZX_ERR_NOT_FOUND;
    }
    nid[i + 1] = pages[i].GetPage<NodePage>().GetNid(offset[i + 1]);
  }

  // free direct nodes linked to a partial indirect node
  for (auto i = offset[idx + 1]; i < kNidsPerBlock; ++i) {
    nid_t child_nid = pages[idx].GetPage<NodePage>().GetNid(i);
    if (!child_nid) {
      continue;
    }
    if (auto ret = TruncateDnode(child_nid); ret.is_error()) {
      return ret.error_value();
    }
    ZX_ASSERT(!pages[idx].GetPage<NodePage>().IsInode());
    pages[idx].WaitOnWriteback();
    pages[idx].GetPage<NodePage>().SetNid(i, 0);
    pages[idx].SetDirty();
  }

  if (offset[idx + 1] == 0) {
    TruncateNode(pages[idx]);
  }
  return ZX_OK;
}

// All the block addresses of data and nodes should be nullified.
zx_status_t VnodeF2fs::TruncateInodeBlocks(pgoff_t from) {
  auto node_path = GetNodePath(from);
  if (node_path.is_error()) {
    return node_path.error_value();
  }

  const size_t level = node_path->depth;
  const size_t (&node_offsets)[kMaxNodeBlockLevel] = node_path->node_offset;
  size_t (&offsets_in_node)[kMaxNodeBlockLevel] = node_path->offset_in_node;
  size_t node_offset = 0;

  LockedPage locked_ipage;
  if (zx_status_t ret = fs()->GetNodeManager().GetNodePage(Ino(), &locked_ipage); ret != ZX_OK) {
    return ret;
  }
  locked_ipage.WaitOnWriteback();
  Inode& inode = locked_ipage->GetAddress<Node>()->i;
  switch (level) {
    case 0:
      // |from| is held in the inode itself, so every node below it goes. The loop walks the
      // i_nid[] slots, which start at kNodeDir1Block rather than at a file offset.
      node_offset = 1;
      offsets_in_node[0] = kNodeDir1Block;
      offsets_in_node[1] = 0;
      break;
    case 1:
      node_offset = node_offsets[1];
      break;
    case 2:
      node_offset = node_offsets[1];
      if (!offsets_in_node[1]) {
        break;
      }
      if (zx_status_t ret = TruncatePartialNodes(inode, offsets_in_node, level);
          ret != ZX_OK && ret != ZX_ERR_NOT_FOUND) {
        return ret;
      }
      ++offsets_in_node[level - 2];
      offsets_in_node[level - 1] = 0;
      node_offset += 1 + kNidsPerBlock;
      break;
    case 3:
      node_offset = 5 + 2 * kNidsPerBlock;
      if (!offsets_in_node[2]) {
        break;
      }
      if (zx_status_t ret = TruncatePartialNodes(inode, offsets_in_node, level);
          ret != ZX_OK && ret != ZX_ERR_NOT_FOUND) {
        return ret;
      }
      ++offsets_in_node[level - 2];
      offsets_in_node[level - 1] = 0;
      break;
    default:
      ZX_ASSERT(0);
  }

  bool run = true;
  while (run) {
    zx::result<size_t> freed;
    nid_t nid = LeToCpu(inode.i_nid[offsets_in_node[0] - kNodeDir1Block]);
    switch (offsets_in_node[0]) {
      case kNodeDir1Block:
      case kNodeDir2Block:
        freed = TruncateDnode(nid);
        break;

      case kNodeInd1Block:
      case kNodeInd2Block:
        freed = TruncateNodes(nid, node_offset, offsets_in_node[1], 2);
        break;

      case kNodeDIndBlock:
        freed = TruncateNodes(nid, node_offset, offsets_in_node[1], 3);
        run = false;
        break;

      default:
        ZX_ASSERT(0);
    }
    if (freed.is_error()) {
      ZX_DEBUG_ASSERT(freed.error_value() != ZX_ERR_NOT_FOUND);
      return freed.error_value();
    }
    if (offsets_in_node[1] == 0) {
      inode.i_nid[offsets_in_node[0] - kNodeDir1Block] = 0;
      locked_ipage.SetDirty();
    }
    offsets_in_node[1] = 0;
    ++offsets_in_node[0];
    node_offset += *freed;
  }
  return ZX_OK;
}

zx_status_t VnodeF2fs::PurgeInodeBlock() {
  LockedPage ipage;
  if (zx_status_t err = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); err != ZX_OK) {
    return err;
  }

  if (xattr_nid_ > 0) {
    LockedPage page;
    if (zx_status_t err = fs()->GetNodeManager().GetNodePage(xattr_nid_, &page); err != ZX_OK) {
      return err;
    }
    xattr_nid_ = 0;
    TruncateNode(page);
  }
  TruncateNode(ipage);
  return ZX_OK;
}

zx_status_t VnodeF2fs::InitInodeMetadata() {
  std::lock_guard lock(mutex_);
  return InitInodeMetadataUnsafe();
}

zx_status_t VnodeF2fs::InitInodeMetadataUnsafe() {
  LockedPage ipage;
  if (TestFlag(InodeInfoFlag::kNewInode)) {
    zx::result page = NewInodePage();
    if (page.is_error()) {
      return page.error_value();
    }
    ipage = *std::move(page);
#if 0  // porting needed
    // err = f2fs_init_acl(inode, dir);
    // if (err) {
    //   remove_inode_page(inode);
    //   return err;
    // }
#endif
  } else {
    if (zx_status_t err = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); err != ZX_OK) {
      return err;
    }
    ipage.WaitOnWriteback();
  }
  // copy name info. to this inode page
  Inode& inode = ipage->GetAddress<Node>()->i;
  std::string_view name = std::string_view(name_);
  ZX_DEBUG_ASSERT(IsValidNameLength(name));
  uint32_t size = safemath::checked_cast<uint32_t>(name.size());
  inode.i_namelen = CpuToLe(size);
  name.copy(reinterpret_cast<char*>(&inode.i_name[kCurrentBitPos]), size);
  ipage.SetDirty();

  if (TestFlag(InodeInfoFlag::kIncLink)) {
    IncrementLinkUnsafe();
    SetDirty();
  }
  return ZX_OK;
}

zx::result<LockedPage> VnodeF2fs::NewInodePage() {
  if (TestFlag(InodeInfoFlag::kNoAlloc)) {
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }
  // allocate inode page for new inode
  zx::result page = fs()->GetNodeManager().NewNodePage(Ino(), Ino(), IsDir(), 0);
  if (page.is_error()) {
    return page.take_error();
  }
  SetDirty();
  return zx::ok(std::move(*page));
}

// TODO: Consider using a global lock as below
// if (!IsDir())
//   mutex_lock(&superblock_info->writepages);
// Writeback()
// if (!IsDir())
//   mutex_unlock(&superblock_info->writepages);
// fs()->RemoveDirtyDirInode(this);
pgoff_t VnodeF2fs::Writeback(WritebackOperation& operation) {
  pgoff_t nwritten = 0;
  std::vector<fbl::RefPtr<Page>> pages = file_cache_->FindDirtyPages(operation);
  pgoff_t last_key = pages.size() ? pages.back()->GetKey() : operation.end;
  PageList pages_to_disk;
  for (auto& page : pages) {
    // GetBlockAddr() returns kNullAddr when |page| is invalidated before |locked_page|.
    LockedPage locked_page(std::move(page));
    locked_page.WaitOnWriteback();
    block_t addr = GetBlockAddr(locked_page);
    ZX_DEBUG_ASSERT(addr != kNewAddr);
    if (addr == kNullAddr) {
      locked_page.release();
      continue;
    }
    locked_page.SetWriteback(addr);
    if (operation.page_cb) {
      // |page_cb| conducts additional process for the last page of node and meta vnodes.
      operation.page_cb(locked_page.CopyRefPtr(), locked_page->GetKey() == last_key);
    }
    pages_to_disk.push_back(locked_page.release());
    ++nwritten;

    if (!(nwritten % kDefaultBlocksPerSegment)) {
      fs()->GetWriter().ScheduleWriteBlocks(nullptr, std::move(pages_to_disk));
    }
  }
  if (!pages_to_disk.is_empty() || operation.bSync) {
    sync_completion_t completion;
    fs()->GetWriter().ScheduleWriteBlocks(operation.bSync ? &completion : nullptr,
                                          std::move(pages_to_disk), operation.bSync);
    if (operation.bSync) {
      sync_completion_wait(&completion, ZX_TIME_INFINITE);
    }
  }
  return nwritten;
}

void VnodeF2fs::CleanupCache() {
  file_cache_->EvictCleanPages();
  vmo_manager_->Reset();
  dir_entry_cache_.Reset();
}

// Set multimedia files as cold files for hot/cold data separation
void VnodeF2fs::SetColdFile() {
  fs::SharedLock lock(mutex_);
  const std::vector<std::string>& extension_list = superblock_info_.GetExtensionList();
  for (const auto& extension : extension_list) {
    if (std::string_view(name_).ends_with(std::string_view(extension))) {
      SetAdvise(FAdvise::kCold);
      break;
    }
    // compare upper case
    std::string upper_sub(extension);
    std::transform(upper_sub.cbegin(), upper_sub.cend(), upper_sub.begin(), ::toupper);
    if (std::string_view(name_).ends_with(std::string_view(upper_sub))) {
      SetAdvise(FAdvise::kCold);
      break;
    }
  }
}

bool VnodeF2fs::IsColdFile() { return IsAdviseSet(FAdvise::kCold); }

zx_status_t VnodeF2fs::SetExtendedAttribute(XattrIndex index, std::string_view name,
                                            std::span<const uint8_t> value, XattrOption option) {
  if (name.empty()) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (name.length() > kMaxNameLen || value.size() > kMaxXattrValueLength) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  LockedPage xattr_page;
  if (xattr_nid_ > 0) {
    if (zx_status_t err = fs()->GetNodeManager().GetNodePage(xattr_nid_, &xattr_page);
        err != ZX_OK) {
      return err;
    }
  }

  LockedPage ipage;
  if (TestFlag(InodeInfoFlag::kInlineXattr)) {
    if (zx_status_t err = fs()->GetNodeManager().GetNodePage(ino_, &ipage); err != ZX_OK) {
      return err;
    }
  }

  XattrOperator xattr_operator(ipage, xattr_page);

  zx::result<uint32_t> offset = xattr_operator.FindSlotOffset(index, name);

  if (option == XattrOption::kCreate) {
    if (offset.is_ok()) {
      return ZX_ERR_ALREADY_EXISTS;
    }
  }

  if (option == XattrOption::kReplace) {
    if (offset.is_error()) {
      return ZX_ERR_NOT_FOUND;
    }
  }

  if (offset.is_ok()) {
    xattr_operator.Remove(*offset);
  }

  if (!value.empty()) {
    if (zx_status_t err = xattr_operator.Add(index, name, value); err != ZX_OK) {
      return err;
    }
  }

  uint32_t xattr_block_start_offset =
      TestFlag(InodeInfoFlag::kInlineXattr) ? kInlineXattrAddrs : kXattrHeaderSlots;
  if (xattr_nid_ == 0 && xattr_operator.GetEndOffset() > xattr_block_start_offset) {
    zx::result<nid_t> nid = fs()->GetNodeManager().AllocNid();
    if (nid.is_error()) {
      return ZX_ERR_NO_SPACE;
    }
    xattr_nid_ = *nid;

    zx::result<LockedPage> page = fs()->GetNodeManager().NewNodePage(ino_, xattr_nid_, IsDir(), 0);
    if (page.is_error()) {
      fs()->GetNodeManager().AddFreeNid(xattr_nid_);
      xattr_nid_ = 0;
      return page.error_value();
    }
    xattr_page = std::move(*page);

    AddBlocks(1);
    SetDirty();
  } else if (xattr_nid_ > 0 && xattr_operator.GetEndOffset() <= xattr_block_start_offset) {
    TruncateNode(xattr_page);
    xattr_nid_ = 0;
    xattr_page.reset();
    SetDirty();
  }

  xattr_operator.WriteTo(ipage, xattr_page);

  return ZX_OK;
}

zx::result<size_t> VnodeF2fs::GetExtendedAttribute(XattrIndex index, std::string_view name,
                                                   std::span<uint8_t> out) {
  if (name.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (name.length() > kMaxNameLen) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  if (xattr_nid_ == 0) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  LockedPage xattr_page;
  if (zx_status_t err = fs()->GetNodeManager().GetNodePage(xattr_nid_, &xattr_page); err != ZX_OK) {
    return zx::error(err);
  }

  LockedPage ipage;
  if (TestFlag(InodeInfoFlag::kInlineXattr)) {
    if (zx_status_t err = fs()->GetNodeManager().GetNodePage(ino_, &ipage); err != ZX_OK) {
      return zx::error(err);
    }
  }

  XattrOperator xattr_operator(ipage, xattr_page);

  return xattr_operator.Lookup(index, name, out);
}

zx::result<NodePath> VnodeF2fs::GetNodePath(pgoff_t block) {
  const pgoff_t direct_index = GetAddrsPerInode();
  const pgoff_t direct_blks = kAddrsPerBlock;
  const pgoff_t dptrs_per_blk = kNidsPerBlock;
  const pgoff_t indirect_blks =
      safemath::CheckMul(safemath::checked_cast<pgoff_t>(kAddrsPerBlock), kNidsPerBlock)
          .ValueOrDie();
  const pgoff_t dindirect_blks = indirect_blks * kNidsPerBlock;
  NodePath path;
  size_t& level = path.depth;
  auto& offset = path.offset_in_node;
  auto& noffset = path.node_offset;
  size_t n = 0;
  path.ino = Ino();

  noffset[0] = 0;
  if (block < direct_index) {
    offset[n++] = static_cast<int>(block);
    level = 0;
    return zx::ok(path);
  }
  block -= direct_index;
  if (block < direct_blks) {
    offset[n++] = kNodeDir1Block;
    noffset[n] = 1;
    offset[n++] = static_cast<int>(block);
    level = 1;
    return zx::ok(path);
  }
  block -= direct_blks;
  if (block < direct_blks) {
    offset[n++] = kNodeDir2Block;
    noffset[n] = 2;
    offset[n++] = static_cast<int>(block);
    level = 1;
    return zx::ok(path);
  }
  block -= direct_blks;
  if (block < indirect_blks) {
    offset[n++] = kNodeInd1Block;
    noffset[n] = 3;
    offset[n++] = static_cast<int>(block / direct_blks);
    noffset[n] = 4 + offset[n - 1];
    offset[n++] = safemath::checked_cast<int32_t>(
        safemath::CheckMod<pgoff_t>(block, direct_blks).ValueOrDie());
    level = 2;
    return zx::ok(path);
  }
  block -= indirect_blks;
  if (block < indirect_blks) {
    offset[n++] = kNodeInd2Block;
    noffset[n] = 4 + dptrs_per_blk;
    offset[n++] = safemath::checked_cast<int32_t>(block / direct_blks);
    noffset[n] = 5 + dptrs_per_blk + offset[n - 1];
    offset[n++] = safemath::checked_cast<int32_t>(
        safemath::CheckMod<pgoff_t>(block, direct_blks).ValueOrDie());
    level = 2;
    return zx::ok(path);
  }
  block -= indirect_blks;
  if (block < dindirect_blks) {
    offset[n++] = kNodeDIndBlock;
    noffset[n] = 5 + (dptrs_per_blk * 2);
    offset[n++] = static_cast<int>(block / indirect_blks);
    noffset[n] = 6 + (dptrs_per_blk * 2) + offset[n - 1] * (dptrs_per_blk + 1);
    offset[n++] = safemath::checked_cast<int32_t>((block / direct_blks) % dptrs_per_blk);
    noffset[n] = 7 + (dptrs_per_blk * 2) + offset[n - 2] * (dptrs_per_blk + 1) + offset[n - 1];
    offset[n++] = safemath::checked_cast<int32_t>(
        safemath::CheckMod<pgoff_t>(block, direct_blks).ValueOrDie());
    level = 3;
    return zx::ok(path);
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

zx_status_t VnodeF2fs::RecoverInode(NodePage& node_page) {
  {
    std::lock_guard lock(mutex_);
    Inode& inode = node_page.GetAddress<Node>()->i;
    SetMode(LeToCpu(inode.i_mode));
    SetTime<Timestamps::AccessTime>({static_cast<time_t>(LeToCpu(inode.i_atime)),
                                     static_cast<time_t>(LeToCpu(inode.i_atime_nsec))});
    SetTime<Timestamps::BirthTime>({static_cast<time_t>(LeToCpu(inode.i_ctime)),
                                    static_cast<time_t>(LeToCpu(inode.i_ctime_nsec))});
    SetTime<Timestamps::ModificationTime>({static_cast<time_t>(LeToCpu(inode.i_mtime)),
                                           static_cast<time_t>(LeToCpu(inode.i_mtime_nsec))});
  }

  if (!node_page.IsDentDnode()) {
    return ZX_OK;
  }

  zx::result parent = fs()->GetVnode(LeToCpu(node_page.GetAddress<Node>()->i.i_pino));
  if (parent.is_error()) {
    return parent.status_value();
  }
  return fbl::RefPtr<Dir>::Downcast(*parent)->RecoverLink(*this).status_value();
}

zx_status_t VnodeF2fs::CloseNode() {
  fs::SharedLock lock(f2fs::GetGlobalLock());
  {
    fs::SharedLock file_lock(mutex_);
    if (open_count() || GetLinkCountUnsafe()) {
      return ZX_OK;
    }
  }
  Purge();
  return ZX_OK;
}

void VnodeF2fs::Purge() {
  std::lock_guard lock(mutex_);
  return PurgeUnsafe();
}

void VnodeF2fs::PurgeUnsafe() {
  SetFlag(InodeInfoFlag::kNoAlloc);
  ClearDirty();
  PurgeDataBlocks();
  PurgeInodeBlock();
  fs()->GetVCache().Evict(this);
}

}  // namespace f2fs
