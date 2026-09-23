// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/stat.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/dir.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

zx::result<> Dir::RecoverLink(VnodeF2fs &vnode) {
  std::lock_guard dir_lock(mutex_);
  fbl::RefPtr<Page> page;
  auto dir_entry = FindEntry(vnode.GetNameView(), &page);
  if (dir_entry.is_error()) {
    AddLink(vnode.GetNameView(), &vnode);
  } else if (vnode.Ino() != dir_entry->ino) {
    // Remove old dentry
    zx::result old_vnode = fs()->GetVnode(dir_entry->ino);
    if (old_vnode.is_error()) {
      return old_vnode.take_error();
    }
    DeleteEntry(*dir_entry, page, (*old_vnode).get());
    ZX_ASSERT(AddLink(vnode.GetNameView(), &vnode) == ZX_OK);
  }
  return zx::ok();
}

zx_status_t Dir::Link(std::string_view name, fbl::RefPtr<fs::Vnode> new_child) {
  if (GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag)) {
    return ZX_ERR_BAD_STATE;
  }

  if (!fs::IsValidName(name)) {
    return ZX_ERR_INVALID_ARGS;
  }

  fs()->BalanceFs(kMaxNeededBlocksForUpdate);
  fs::SharedLock lock(f2fs::GetGlobalLock());
  fbl::RefPtr<VnodeF2fs> target = fbl::RefPtr<VnodeF2fs>::Downcast(std::move(new_child));
  if (target->IsDir()) {
    return ZX_ERR_NOT_FILE;
  }

  std::lock_guard dir_lock(mutex_);
  if (LookUpEntries(name).is_ok()) {
    return ZX_ERR_ALREADY_EXISTS;
  }

  target->SetFlag(InodeInfoFlag::kIncLink);
  if (zx_status_t err = AddLink(name, target.get()); err != ZX_OK) {
    target->ClearFlag(InodeInfoFlag::kIncLink);
    return err;
  }

  return ZX_OK;
}

zx_status_t Dir::DoLookup(std::string_view name, fbl::RefPtr<fs::Vnode> *out) {
  if (!fs::IsValidName(name)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (zx::result ino = LookUpEntries(name); ino.is_ok()) {
    zx::result vnode = fs()->GetVnode(*ino);
    if (vnode.is_error()) {
      return vnode.error_value();
    }
    *out = *std::move(vnode);
    return ZX_OK;
  }
  return ZX_ERR_NOT_FOUND;
}

zx_status_t Dir::Lookup(std::string_view name, fbl::RefPtr<fs::Vnode> *out) {
  fs::SharedLock dir_read_lock(mutex_);
  return DoLookup(name, out);
}

zx_status_t Dir::DoUnlink(VnodeF2fs *vnode, std::string_view name) {
  fbl::RefPtr<Page> page;
  auto entry_info = FindEntry(name, &page);
  if (entry_info.is_error()) {
    return ZX_ERR_NOT_FOUND;
  }
  if (zx_status_t err = fs()->CheckOrphanSpace(); err != ZX_OK) {
    return err;
  }

  DeleteEntry(*entry_info, page, vnode);
  return ZX_OK;
}

zx_status_t Dir::Rmdir(Dir *vnode, std::string_view name) {
  if (vnode->IsEmptyDir()) {
    return DoUnlink(vnode, name);
  }
  return ZX_ERR_NOT_EMPTY;
}

#if 0  // porting needed
// int Dir::F2fsSymlink(dentry *dentry, const char *symname) {
//   return 0;
//   //   fbl::RefPtr<VnodeF2fs> vnode_refptr;
//   //   VnodeF2fs *vnode = nullptr;
//   //   unsigned symlen = strlen(symname) + 1;
//   //   int err;

//   //   err = NewInode(S_IFLNK | S_IRWXUGO, &vnode_refptr);
//   //   if (err)
//   //     return err;
//   //   vnode = vnode_refptr.get();

//   //   // inode->i_mapping->a_ops = &f2fs_dblock_aops;

//   //   // err = AddLink(dentry, vnode);
//   //   if (err)
//   //     goto out;

//   //   err = page_symlink(vnode, symname, symlen);
//   //   fs()->GetNodeManager().AllocNidDone(vnode->Ino());

//   //   // d_instantiate(dentry, vnode);
//   //   UnlockNewInode(vnode);

//   //   fs()->GetSegmentManager().BalanceFs();

//   //   return err;
//   // out:
//   //   vnode->ClearLinkCount();
//   //   UnlockNewInode(vnode);
//   //   fs()->GetNodeManager().AllocNidFailed(vnode->Ino());
//   //   return err;
// }

// int Dir::F2fsMknod(dentry *dentry, umode_t mode, dev_t rdev) {
//   fbl::RefPtr<VnodeF2fs> vnode_refptr;
//   VnodeF2fs *vnode = nullptr;
//   int err = 0;

//   // if (!new_valid_dev(rdev))
//   //   return -EINVAL;

//   err = NewInode(mode, &vnode_refptr);
//   if (err)
//     return err;
//   vnode = vnode_refptr.get();

//   // init_special_inode(inode, inode->i_mode, rdev);
//   // inode->i_op = &f2fs_special_inode_operations;

//   // err = AddLink(dentry, vnode);
//   if (err)
//     goto out;

//   fs()->GetNodeManager().AllocNidDone(vnode->Ino());
//   // d_instantiate(dentry, inode);
//   UnlockNewInode(vnode);

//   fs()->GetSegmentManager().BalanceFs();

//   return 0;
// out:
//   vnode->ClearLinkCount();
//   UnlockNewInode(vnode);
//   fs()->GetNodeManager().AllocNidFailed(vnode->Ino());
//   return err;
// }
#endif

zx::result<bool> Dir::IsSubdir(Dir *possible_dir) {
  const nid_t root_ino = GetSuperblockInfo().GetRootIno();
  if (Ino() == root_ino) {
    return zx::ok(true);
  }

  VnodeF2fs *vnode = possible_dir;
  fbl::RefPtr<VnodeF2fs> current;

  while (vnode->Ino() != root_ino) {
    if (vnode->Ino() == Ino()) {
      return zx::ok(true);
    }

    zx::result parent = fs()->GetVnode(vnode->GetParentNid());
    if (parent.is_error()) {
      return parent.take_error();
    }
    current = std::move(*parent);
    vnode = current.get();
  }
  return zx::ok(false);
}

zx_status_t Dir::Rename(fbl::RefPtr<fs::Vnode> _newdir, std::string_view oldname,
                        std::string_view newname, bool src_must_be_dir, bool dst_must_be_dir) {
  if (GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag)) {
    return ZX_ERR_BAD_STATE;
  }

  fs()->BalanceFs(2 * kMaxNeededBlocksForUpdate + 1);
  {
    fs::SharedLock lock(f2fs::GetGlobalLock());
    fbl::RefPtr<Dir> new_dir = fbl::RefPtr<Dir>::Downcast(std::move(_newdir));
    bool is_same_dir = (new_dir.get() == this);
    if (!fs::IsValidName(oldname) || !fs::IsValidName(newname)) {
      return ZX_ERR_INVALID_ARGS;
    }

    std::lock_guard dir_lock(mutex_);
    if ((is_same_dir && !GetLinkCountUnsafe()) || (!is_same_dir && !new_dir->GetLinkCount())) {
      return ZX_ERR_NOT_FOUND;
    }

    fbl::RefPtr<Page> old_page;
    auto old_entry = FindEntry(oldname, &old_page);
    if (old_entry.is_error()) {
      return ZX_ERR_NOT_FOUND;
    }

    nid_t old_ino = old_entry->ino;
    zx::result old_vnode = fs()->GetVnode(old_ino);
    if (old_vnode.is_error()) {
      return old_vnode.error_value();
    }

    ZX_DEBUG_ASSERT(old_vnode->IsSameName(oldname));

    if (!old_vnode->IsDir() && (src_must_be_dir || dst_must_be_dir)) {
      return ZX_ERR_NOT_DIR;
    }

    ZX_DEBUG_ASSERT(!src_must_be_dir || old_vnode->IsDir());

    fbl::RefPtr<Page> old_dir_page;
    zx::result<DentryInfo> old_dir_entry = zx::error(ZX_ERR_NOT_FOUND);
    if (old_vnode->IsDir()) {
      old_dir_entry = fbl::RefPtr<Dir>::Downcast(*old_vnode)->GetParentDentryInfo(&old_dir_page);
      if (old_dir_entry.is_error()) {
        return ZX_ERR_IO;
      }

      auto is_subdir = fbl::RefPtr<Dir>::Downcast(*old_vnode)->IsSubdir(new_dir.get());
      if (is_subdir.is_error()) {
        return is_subdir.error_value();
      }
      if (*is_subdir) {
        return ZX_ERR_INVALID_ARGS;
      }
    }

    fbl::RefPtr<Page> new_page;
    zx::result<DentryInfo> new_entry = zx::error(ZX_ERR_NOT_FOUND);
    if (is_same_dir) {
      new_entry = FindEntry(newname, &new_page);
    } else {
      new_entry = new_dir->FindEntrySafe(newname, &new_page);
    }

    if (new_entry.is_ok()) {
      ino_t new_ino = new_entry->ino;
      zx::result new_vnode = fs()->GetVnode(new_ino);
      if (new_vnode.is_error()) {
        return new_vnode.error_value();
      }

      if (!new_vnode->IsDir() && (src_must_be_dir || dst_must_be_dir)) {
        return ZX_ERR_NOT_DIR;
      }

      if (old_vnode->IsDir() && !new_vnode->IsDir()) {
        return ZX_ERR_NOT_DIR;
      }

      if (!old_vnode->IsDir() && new_vnode->IsDir()) {
        return ZX_ERR_NOT_FILE;
      }

      if (is_same_dir && oldname == newname) {
        return ZX_OK;
      }

      if (old_dir_entry.is_ok() &&
          (!new_vnode->IsDir() || !fbl::RefPtr<Dir>::Downcast(*new_vnode)->IsEmptyDir())) {
        return ZX_ERR_NOT_EMPTY;
      }

      ZX_DEBUG_ASSERT(this != (*new_vnode).get());
      ZX_DEBUG_ASSERT(new_vnode->IsSameName(newname));
      old_vnode->SetName(newname);
      if (is_same_dir) {
        SetLink(*new_entry, new_page, (*old_vnode).get());
      } else {
        new_dir->SetLinkSafe(*new_entry, new_page, (*old_vnode).get());
      }

      if (old_dir_entry.is_ok()) {
        new_vnode->DecrementLink();
      }
      new_vnode->DecrementLink();
      if (!new_vnode->GetLinkCount()) {
        new_vnode->SetOrphan();
      }
      new_vnode->SetDirty();
    } else {
      if (is_same_dir && oldname == newname) {
        return ZX_OK;
      }

      old_vnode->SetName(newname);

      if (is_same_dir) {
        if (zx_status_t err = AddLink(newname, (*old_vnode).get()); err != ZX_OK) {
          return err;
        }
        if (old_dir_entry.is_ok()) {
          IncrementLinkUnsafe();
          SetDirty();
        }
      } else {
        if (zx_status_t err = new_dir->AddLinkSafe(newname, (*old_vnode).get()); err != ZX_OK) {
          return err;
        }
        if (old_dir_entry.is_ok()) {
          new_dir->IncrementLink();
          new_dir->SetDirty();
        }
      }
    }

    old_vnode->SetParentNid(new_dir->Ino());
    old_vnode->SetFlag(InodeInfoFlag::kNeedCp);
    old_vnode->SetDirty();

    DeleteEntry(*old_entry, old_page, nullptr);

    if (old_dir_entry.is_ok()) {
      if (!is_same_dir) {
        fbl::RefPtr<Dir>::Downcast(*old_vnode)
            ->SetLinkSafe(*old_dir_entry, old_dir_page, new_dir.get());
      }
      DecrementLinkUnsafe();
      SetDirty();
    }

    // Add new parent directory to VnodeSet to ensure consistency of renamed vnode.
    fs()->AddToVnodeSet(VnodeSet::kModifiedDir, new_dir->Ino());
    if (old_vnode->IsDir()) {
      fs()->AddToVnodeSet(VnodeSet::kModifiedDir, old_vnode->Ino());
    }
  }

  return ZX_OK;
}

zx::result<fbl::RefPtr<fs::Vnode>> Dir::Create(std::string_view name, fs::CreationType type) {
  switch (type) {
    case fs::CreationType::kDirectory:
      return CreateWithMode(name, S_IFDIR);
    case fs::CreationType::kFile:
      return CreateWithMode(name, S_IFREG);
  }
  return zx::error(ZX_ERR_INVALID_ARGS);
}

zx::result<fbl::RefPtr<fs::Vnode>> Dir::CreateWithMode(std::string_view name, umode_t mode) {
  if (GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag)) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (!fs::IsValidName(name)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fs()->BalanceFs(kMaxNeededBlocksForUpdate + 1);

  fs::SharedLock lock(f2fs::GetGlobalLock());
  bool is_dir = S_ISDIR(mode);
  fbl::RefPtr<VnodeF2fs> new_vnode;
  auto purge = fit::defer([&]() TA_NO_THREAD_SAFETY_ANALYSIS {
    if (!new_vnode) {
      return;
    }
    new_vnode->ClearFlag(InodeInfoFlag::kIncLink);
    new_vnode->ClearLinkCount();
    new_vnode->Purge();
    fs()->GetNodeManager().AddFreeNid(new_vnode->Ino());
  });

  std::lock_guard dir_lock(mutex_);
  if (!GetLinkCountUnsafe()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  if (LookUpEntries(name).is_ok()) {
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  zx::result vnode = fs()->CreateNewVnode(mode);
  if (vnode.is_error()) {
    return vnode.take_error();
  }

  vnode->SetName(name);
  if (is_dir) {
    vnode->SetFlag(InodeInfoFlag::kIncLink);
  } else if (!GetSuperblockInfo().TestOpt(MountOption::kDisableExtIdentify)) {
    vnode->SetColdFile();
  }

  new_vnode = std::move(*vnode);
  if (zx_status_t err = AddLink(name, new_vnode.get()); err != ZX_OK) {
    return zx::error(err);
  }

  new_vnode->ClearFlag(InodeInfoFlag::kNewInode);
  new_vnode->ResetTime();
  purge.cancel();
  return zx::make_result(new_vnode->Open(nullptr), new_vnode);
}

zx_status_t Dir::Unlink(std::string_view name, bool must_be_dir) {
  if (GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag)) {
    return ZX_ERR_BAD_STATE;
  }
  //|vnode| should be purged while holding f2fs::GetGlobalLock().
  fs::SharedLock lock(f2fs::GetGlobalLock());
  std::lock_guard dir_lock(mutex_);
  fbl::RefPtr<fs::Vnode> vnode;
  if (zx_status_t status = DoLookup(name, &vnode); status != ZX_OK) {
    return status;
  }
  fbl::RefPtr<VnodeF2fs> removed = fbl::RefPtr<VnodeF2fs>::Downcast(std::move(vnode));
  zx_status_t ret;
  if (removed->IsDir()) {
    fbl::RefPtr<Dir> dir = fbl::RefPtr<Dir>::Downcast(std::move(removed));
    ret = Rmdir(dir.get(), name);
  } else if (must_be_dir) {
    return ZX_ERR_NOT_DIR;
  } else {
    ret = DoUnlink(removed.get(), name);
  }
  return ret;
}

}  // namespace f2fs
