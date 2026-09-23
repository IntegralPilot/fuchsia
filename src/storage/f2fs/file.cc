// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/file.h"

#include <zircon/syscalls-next.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

File::File(F2fs *fs, ino_t ino, umode_t mode, LockedPage node_page)
    : VnodeF2fs(fs, ino, mode, std::move(node_page)) {}

File::File(F2fs *fs, ino_t ino, umode_t mode, std::optional<gid_t> gid) : VnodeF2fs(fs, ino, mode) {
  if (gid) {
    SetGid(*gid);
  }
}

#if 0  // porting needed
// int File::F2fsVmPageMkwrite(vm_area_struct *vma, vm_fault *vmf) {
//   return 0;
//   //   Page *page = vmf->page;
//   //   VnodeF2fs *vnode = this;
//   //   // SuperblockInfo &superblock_info = Vsuperblock_info_;
//   //   Page *node_page;
//   //   block_t old_blk_addr;
//   //   DnodeOfData dn;
//   //   int err;

//   //   Vfs()->GetSegmentManager().BalanceFs();

//   //   sb_start_pagefault(nullptr);

//   //   // superblock_info->mutex_lock_op(LockType::kDataNew);

//   //   /* block allocation */
//   //   SetNewDnode(&dn, vnode, nullptr, nullptr, 0);
//   //   err = Vfs()->GetNodeManager().GetDnodeOfData(&dn, page->index, 0);
//   //   if (err) {
//   //     // superblock_info->mutex_unlock_op(LockType::kDataNew);
//   //     goto out;
//   //   }

//   //   old_blk_addr = dn.data_blkaddr;
//   //   node_page = dn.node_page;

//   //   if (old_blk_addr == kNullAddr) {
//   //     err = VnodeF2fs::ReserveNewBlock(&dn);
//   //     if (err) {
//   //       F2fsPutDnode(&dn);
//   //       // superblock_info->mutex_unlock_op(LockType::kDataNew);
//   //       goto out;
//   //     }
//   //   }
//   //   F2fsPutDnode(&dn);

//   //   // superblock_info->mutex_unlock_op(LockType::kDataNew);

//   //   // lock_page(page);
//   //   // if (page->mapping != inode->i_mapping ||
//   //   //     page_offset(page) >= i_size_read(inode) ||
//   //   //     !PageUptodate(page)) {
//   //   //   unlock_page(page);
//   //   //   err = -EFAULT;
//   //   //   goto out;
//   //   // }

//   //   /*
//   //    * check to see if the page is mapped already (no holes)
//   //    */
//   //   if (PageMappedToDisk(page))
//   //     goto out;

//   //   /* fill the page */
//   //   WaitOnPageWriteback(page);

//   //   /* page is wholly or partially inside EOF */
//   //   if (((page->index + 1) << kPageCacheShift) > i_size) {
//   //     unsigned offset;
//   //     offset = i_size & ~PAGE_CACHE_MASK;
//   //     ZeroUserSegment(page, offset, kBlockSize);
//   //   }
//   //   // set_page_dirty(page);
//   //   FlushDirtyDataPage(Vfs(), *page);
//   //   SetPageUptodate(page);

//   //   file_update_time(vma->vm_file);
//   // out:
//   //   sb_end_pagefault(nullptr);
//   //   return block_page_mkwrite_return(err);
// }
#endif

#if 0  // porting needed
// void File::FillZero(pgoff_t index, loff_t start, loff_t len) {
//   Page *page = nullptr;
//   zx_status_t err;

//   if (!len)
//     return;

//   err = GetNewDataPage(index, false, page);

//   if (!err) {
//     WaitOnPageWriteback(page);
//     zero_user(page, start, len);
// #if 0  // porting needed
//     // set_page_dirty(page);
// #else
//     FlushDirtyDataPage(Vfs(), *page);
// #endif
//     Page::PutPage(page, 1);
//   }
// }

// int File::PunchHole(loff_t offset, loff_t len, int mode) {
//   pgoff_t pg_start, pg_end;
//   loff_t off_start, off_end;
//   int ret = 0;

//   pg_start = ((uint64_t)offset) >> kPageCacheShift;
//   pg_end = ((uint64_t)offset + len) >> kPageCacheShift;

//   off_start = offset & (kBlockSize - 1);
//   off_end = (offset + len) & (kBlockSize - 1);

//   if (pg_start == pg_end) {
//     FillZero(pg_start, off_start, off_end - off_start);
//   } else {
//     if (off_start)
//       FillZero(pg_start++, off_start, kBlockSize - off_start);
//     if (off_end)
//       FillZero(pg_end, 0, off_end);

//     if (pg_start < pg_end) {
//       loff_t blk_start, blk_end;

//       blk_start = pg_start << kPageCacheShift;
//       blk_end = pg_end << kPageCacheShift;
// #if 0  // porting needed
//       // truncate_inode_pages_range(nullptr, blk_start,
//       //     blk_end - 1);
// #endif
//       ret = TruncateHole(pg_start, pg_end);
//     }
//   }

//   if (!(mode & FALLOC_FL_KEEP_SIZE) && i_size <= (offset + len)) {
//     i_size = offset;
//     SetDirty();
//   }

//   return ret;
// }

// int File::ExpandInodeData(loff_t offset, off_t len, int mode) {
//   SuperblockInfo &superblock_info = Vsuperblock_info_;
//   pgoff_t pg_start, pg_end;
//   loff_t new_size = i_size;
//   loff_t off_start, off_end;
//   int ret = 0;

// #if 0  // porting needed
//   // ret = inode_newsize_ok(this, (len + offset));
//   // if (ret)
//   //   return ret;
// #endif

//   pg_start = ((uint64_t)offset) >> kPageCacheShift;
//   pg_end = ((uint64_t)offset + len) >> kPageCacheShift;

//   off_start = offset & (kBlockSize - 1);
//   off_end = (offset + len) & (kBlockSize - 1);

//   for (pgoff_t index = pg_start; index <= pg_end; ++index) {
//     DnodeOfData dn;

//     superblock_info.mutex_lock_op(LockType::kDataNew);

//     SetNewDnode(&dn, this, nullptr, nullptr, 0);
//     ret = Vfs()->GetNodeManager().GetDnodeOfData(&dn, index, 0);
//     if (ret) {
//       superblock_info.mutex_unlock_op(LockType::kDataNew);
//       break;
//     }

//     if (dn.data_blkaddr == kNullAddr) {
//       ret = ReserveNewBlock(&dn);
//       if (ret) {
//         F2fsPutDnode(&dn);
//         superblock_info.mutex_unlock_op(LockType::kDataNew);
//         break;
//       }
//     }
//     F2fsPutDnode(&dn);

//     superblock_info.mutex_unlock_op(LockType::kDataNew);

//     if (pg_start == pg_end)
//       new_size = offset + len;
//     else if (index == pg_start && off_start)
//       new_size = (index + 1) << kPageCacheShift;
//     else if (index == pg_end)
//       new_size = (index << kPageCacheShift) + off_end;
//     else
//       new_size += kBlockSize;
//   }

//   if (!(mode & FALLOC_FL_KEEP_SIZE) && i_size < new_size) {
//     i_size = new_size;
//     SetDirty();
//   }

//   return ret;
// }

// long File::F2fsFallocate(int mode, loff_t offset, loff_t len) {
//   long ret;

//   if (mode & ~(FALLOC_FL_KEEP_SIZE | FALLOC_FL_PUNCH_HOLE))
//     return -EOPNOTSUPP;

//   if (mode & FALLOC_FL_PUNCH_HOLE)
//     ret = PunchHole(offset, len, mode);
//   else
//     ret = ExpandInodeData(offset, len, mode);

//   Vfs()->GetSegmentManager().BalanceFs();
//   return ret;
// }

// #define F2FS_REG_FLMASK    (~(FS_DIRSYNC_FL | FS_TOPDIR_FL))
// #define F2FS_OTHER_FLMASK  (FS_NODUMP_FL | FS_NOATIME_FL)

// inline uint32_t File::F2fsMaskFlags(umode_t mode, uint32_t flags) {
//   if (S_ISDIR(mode))
//     return flags;
//   else if (S_ISREG(mode))
//     return flags & F2FS_REG_FLMASK;
//   else
//     return flags & F2FS_OTHER_FLMASK;
// }

// long File::F2fsIoctl(/*file *filp,*/ unsigned int cmd, uint64_t arg) {
  //   inode *inode = filp->f_dentry->d_inode;
  //   InodeInfo *fi = F2FS_I(inode);
  //   unsigned int flags;
  //   int ret;

  //   switch (cmd) {
  //   case FS_IOC_GETFLAGS:
  //     flags = fi->i_flags & FS_FL_USER_VISIBLE;
  //     return put_user(flags, (int __user *) arg);
  //   case FS_IOC_SETFLAGS:
  //   {
  //     unsigned int oldflags;

  //     ret = mnt_want_write(filp->f_path.mnt);
  //     if (ret)
  //       return ret;

  //     if (!inode_owner_or_capable(inode)) {
  //       ret = -EACCES;
  //       goto out;
  //     }

  //     if (get_user(flags, (int __user *) arg)) {
  //       ret = -EFAULT;
  //       goto out;
  //     }

  //     flags = f2fs_mask_flags(inode->i_mode, flags);

  //     mutex_lock(&inode->i_mutex_);

  //     oldflags = fi->i_flags;

  //     if ((flags ^ oldflags) & (FS_APPEND_FL | FS_IMMUTABLE_FL)) {
  //       if (!capable(CAP_LINUX_IMMUTABLE)) {
  //         mutex_unlock(&inode->i_mutex_);
  //         ret = -EPERM;
  //         goto out;
  //       }
  //     }

  //     flags = flags & FS_FL_USER_MODIFIABLE;
  //     flags |= oldflags & ~FS_FL_USER_MODIFIABLE;
  //     fi->i_flags = flags;
  //     mutex_unlock(&inode->i_mutex_);

  //     f2fs_set_inode_flags(inode);
  //     inode->i_ctime = CURRENT_TIME;
  //     SetDirty(inode);
  // out:
  //     mnt_drop_write(filp->f_path.mnt);
  //     return ret;
  //   }
  //   default:
  //     return -ENOTTY;
  //   }
// }
#endif

zx_status_t File::Truncate(size_t len) {
  TRACE_DURATION("f2fs", "File::Truncate", "event", "File::Truncate", "ino", Ino(), "length", len);

  if (GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag)) {
    return ZX_ERR_BAD_STATE;
  }

  if (len == GetSize()) {
    return ZX_OK;
  }

  if (len > MaxFileSize()) {
    return ZX_ERR_INVALID_ARGS;
  }

  fs::SharedLock lock(f2fs::GetGlobalLock());
  if (TestFlag(InodeInfoFlag::kInlineData) && len >= MaxInlineData()) {
    ConvertInlineData();
  }

  if (TestFlag(InodeInfoFlag::kInlineData)) {
    if (zx_status_t ret = TruncateInline(len); ret != ZX_OK) {
      return ret;
    }
    SetSize(len);
    return ZX_OK;
  }

  {
    std::lock_guard file_lock(mutex_);
    if (zx_status_t ret = DoTruncate(len); ret != ZX_OK) {
      return ret;
    }
    SetTime<Timestamps::ModificationTime>();
  }
  SetSize(len);
  return ZX_OK;
}

size_t File::MaxFileSize() {
  size_t result = GetAddrsPerInode();
  size_t leaf_count = kAddrsPerBlock;

  // two direct node blocks
  result += (leaf_count * 2);

  // two indirect node blocks
  leaf_count *= kNidsPerBlock;
  result += (leaf_count * 2);

  // one double indirect node block
  leaf_count *= kNidsPerBlock;
  result += leaf_count;

  result <<= GetSuperblockInfo().GetLogBlocksize();
  return result;
}

zx_status_t File::GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo *out_vmo) {
  if (flags & fuchsia_io::wire::VmoFlags::kExecute) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // mmapped vnodes are not allowed to have inline data.
  if (TestFlag(InodeInfoFlag::kInlineData)) {
    ConvertInlineData();
  }
  std::lock_guard lock(mutex_);
  return VnodeF2fs::GetVmo(flags, out_vmo);
}

void File::VmoDirty(uint64_t offset, uint64_t length) {
  if (unlikely(offset + length > MaxFileSize())) {
    return ReportPagerError(ZX_PAGER_OP_DIRTY, offset, length, ZX_ERR_BAD_STATE);
  }

  uint32_t num_blocks =
      CheckedDivRoundUp<uint32_t>(safemath::checked_cast<uint32_t>(length), kBlockSize);
  // Touch nothing while |this| is being purged or converting inline data.
  if (unlikely(TestFlag(InodeInfoFlag::kNoAlloc) || TestFlag(InodeInfoFlag::kInlineData))) {
    fs::SharedLock file_lock(mutex_);
    return VnodeF2fs::VmoDirty(offset, length);
  }
  fs()->BalanceFs(num_blocks);

  fs::SharedLock lock(f2fs::GetGlobalLock());
  std::lock_guard file_lock(mutex_);
  zx::result pages = WriteBegin(offset, length);
  if (unlikely(pages.is_error())) {
    return ReportPagerErrorUnsafe(ZX_PAGER_OP_DIRTY, offset, length, pages.error_value());
  }
  SetTime<Timestamps::ModificationTime>();
  SetDirty();
  return VnodeF2fs::VmoDirty(offset, length);
}

void File::VmoRead(uint64_t offset, uint64_t length) {
  ZX_DEBUG_ASSERT(kBlockSize == zx_system_get_page_size());
  ZX_DEBUG_ASSERT(offset % kBlockSize == 0);
  ZX_DEBUG_ASSERT(length);
  ZX_DEBUG_ASSERT(length % kBlockSize == 0);
  fs::SharedLock rlock(mutex_);
  return VnodeF2fs::VmoRead(offset, length);
}

zx::result<zx::stream> File::CreateStream(uint32_t stream_options) {
  if (TestFlag(InodeInfoFlag::kInlineData)) {
    ConvertInlineData();
  }

  std::lock_guard lock(mutex_);
  zx::stream stream;
  if (zx_status_t status = zx::stream::create(stream_options, paged_vmo(), 0u, &stream);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(stream));
}

block_t File::GetBlockAddr(LockedPage &page) { return GetBlockAddrOnDataSegment(page); }

zx::result<LockedPage> File::FindVictimPage(pgoff_t index) {
  LockedPage data_page;
  if (zx_status_t ret = GrabLockedPage(index, &data_page); ret != ZX_OK) {
    return zx::error(ret);
  }

  // Ask kernel to dirty the vmo area for |data_page|. If the regarding pages are not present, we
  // supply a vmo and dirty it again.
  zx::result dirty = data_page.SetVmoDirty();
  if (dirty.is_ok()) {
    return zx::ok(std::move(data_page));
  }
  if (dirty.error_value() != ZX_ERR_NOT_FOUND) {
    return dirty.take_error();
  }
  pgoff_t offset = safemath::CheckMul(data_page->GetKey(), kBlockSize).ValueOrDie();
  VmoRead(offset, Page::Size());
  if (zx::result ret = data_page.SetVmoDirty(); ret.is_error()) {
    return ret.take_error();
  }
  return zx::ok(std::move(data_page));
}

}  // namespace f2fs
