// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/vnode_cache.h"

#include "src/storage/f2fs/vnode.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

VnodeCache::VnodeCache() = default;

VnodeCache::~VnodeCache() { Reset(); }

void VnodeCache::Reset() {
  {
    fs::SharedLock list_lock(list_lock_);
    if (ndirty_ || ndirty_dir_) {
      FX_LOGS(WARNING) << "VnodeCache is being reset while it still contains dirty vnodes. "
                       << ndirty_ << "/" << ndirty_dir_;
    }
  }
  ForDirtyVnodesIf([this](fbl::RefPtr<VnodeF2fs>& vnode) { return RemoveDirty(vnode.get()); });
  ForAllVnodes([this](fbl::RefPtr<VnodeF2fs>& vnode) { return Evict(vnode.get()); });
}

zx_status_t VnodeCache::ForAllVnodes(VnodeCallback callback) {
  ino_t next = 0;
  while (true) {
    fbl::RefPtr<VnodeF2fs> vnode;
    {
      fs::SharedLock lock(table_lock_);
      if (vnode_table_.is_empty()) {
        return ZX_OK;
      }
      // ... Acquire all subsequent nodes by iterating from the lower bound of the current node.
      auto current = vnode_table_.lower_bound(next);
      if (current == vnode_table_.end()) {
        return ZX_OK;
      }
      next = current->GetKey();
      zx::result ref = current->GetRefPtr();
      if (ref.is_ok()) {
        vnode = std::move(*ref);
      } else if (ref.error_value() == ZX_ERR_SHOULD_WAIT) {
        continue;
      }
    }
    ++next;
    if (!callback || !vnode) {
      continue;
    }
    zx_status_t status = callback(vnode);
    if (status == ZX_ERR_STOP) {
      break;
    }
    if (status != ZX_ERR_NEXT && status != ZX_OK) {
      return status;
    }
  }
  return ZX_OK;
}

zx_status_t VnodeCache::ForDirtyVnodesIf(VnodeCallback cb, VnodeCallback cb_if) {
  std::vector<fbl::RefPtr<VnodeF2fs>> dirty_vnodes;
  {
    fs::SharedLock lock(list_lock_);
    for (auto iter = dirty_list_.begin(); iter != dirty_list_.end(); ++iter) {
      fbl::RefPtr<VnodeF2fs> vnode = iter.CopyPointer();
      if (cb_if == nullptr || cb_if(vnode) == ZX_OK) {
        dirty_vnodes.push_back(std::move(vnode));
      }
    }
  }

  if (dirty_vnodes.empty()) {
    return ZX_OK;
  }

  for (auto& vnode : dirty_vnodes) {
    if (zx_status_t status = cb(vnode); status == ZX_ERR_STOP) {
      break;
    } else if (status != ZX_ERR_NEXT && status != ZX_OK) {
      return status;
    }
  }

  return ZX_OK;
}

zx_status_t VnodeCache::Lookup(const ino_t& ino, fbl::RefPtr<VnodeF2fs>* out) {
  while (true) {
    fs::SharedLock lock(table_lock_);
    auto raw_ptr = vnode_table_.find(ino).CopyPointer();
    if (!raw_ptr) {
      break;
    }
    zx::result vnode = raw_ptr->GetRefPtr();
    if (vnode.is_error()) {
      if (vnode.status_value() == ZX_ERR_SHOULD_WAIT) {
        continue;
      }
      return vnode.status_value();
    }
    *out = std::move(*vnode);
    return ZX_OK;
  }
  return ZX_ERR_NOT_FOUND;
}

zx_status_t VnodeCache::Evict(VnodeF2fs* vnode) {
  ZX_ASSERT(!(*vnode).fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>>::InContainer());
  std::lock_guard lock(table_lock_);
  return EvictUnsafe(vnode);
}

zx_status_t VnodeCache::EvictUnsafe(VnodeF2fs* vnode) {
  if (!(*vnode).fbl::WAVLTreeContainable<VnodeF2fs*>::InContainer()) {
    FX_LOGS(INFO) << "EvictUnsafe: " << vnode->GetNameView() << "(" << vnode->GetKey()
                  << ") cannot be found in vnode table";
    return ZX_ERR_NOT_FOUND;
  }
  ZX_ASSERT_MSG(vnode_table_.erase(*vnode) != nullptr, "Cannot find vnode (%u)", vnode->GetKey());
  return ZX_OK;
}

zx_status_t VnodeCache::Add(VnodeF2fs* vnode) {
  std::lock_guard lock(table_lock_);
  if ((*vnode).fbl::WAVLTreeContainable<VnodeF2fs*>::InContainer()) {
    return ZX_ERR_ALREADY_EXISTS;
  }
  vnode_table_.insert(vnode);
  return ZX_OK;
}

zx_status_t VnodeCache::AddDirty(VnodeF2fs& vnode) {
  std::lock_guard lock(list_lock_);
  if (vnode.fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>>::InContainer()) {
    return ZX_ERR_ALREADY_EXISTS;
  }
  fbl::RefPtr<VnodeF2fs> vnode_refptr = fbl::MakeRefPtrUpgradeFromRaw(&vnode, list_lock_);
  dirty_list_.push_back(std::move(vnode_refptr));
  if (vnode.IsDir()) {
    ++ndirty_dir_;
  }
  ++ndirty_;
  return ZX_OK;
}

bool VnodeCache::IsDirty(VnodeF2fs& vnode) {
  fs::SharedLock lock(list_lock_);
  if (vnode.fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>>::InContainer()) {
    return true;
  }
  return false;
}

zx_status_t VnodeCache::RemoveDirty(VnodeF2fs* vnode) {
  std::lock_guard lock(list_lock_);
  return RemoveDirtyUnsafe(vnode);
}

zx_status_t VnodeCache::RemoveDirtyUnsafe(VnodeF2fs* vnode) {
  if (!(*vnode).fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>>::InContainer()) {
    return ZX_ERR_NOT_FOUND;
  }
  auto vnode_refptr = dirty_list_.erase(*vnode);
  if (vnode_refptr->IsDir()) {
    --ndirty_dir_;
  }
  --ndirty_;
  return ZX_OK;
}

void VnodeCache::Shrink() {
  ForAllVnodes([](fbl::RefPtr<VnodeF2fs>& vnode) {
    vnode->CleanupCache();
    return ZX_OK;
  });
  ino_t next = 0;
  // All vnodes in |evicted| are deleted on return via fbl_recycle(), where Vnode::mutex_ is held.
  // To avoid introducing lock-order dependencies between vnodes and the vnode cache,
  // declare |evicted| before |table_lock_| and |list_lock_|.
  std::vector<fbl::RefPtr<VnodeF2fs>> evicted;
  std::lock_guard table_lock(table_lock_);
  std::lock_guard list_lock(list_lock_);
  while (++next) {
    auto current = vnode_table_.lower_bound(next);
    if (current == vnode_table_.end()) {
      break;
    }
    next = current->GetKey();
    if (current->IsActive()) {
      continue;
    }
    zx::result ref = current->GetRefPtr();
    ZX_ASSERT(ref.is_ok());
    if (ref->fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>>::InContainer()) {
      continue;
    }
    EvictUnsafe((*ref).get());
    evicted.push_back(std::move(*ref));
  }
}

}  // namespace f2fs
