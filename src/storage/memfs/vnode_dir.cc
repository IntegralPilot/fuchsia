// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/memfs/vnode_dir.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"
#include "src/storage/memfs/dnode.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode_file.h"
#include "src/storage/memfs/vnode_vmo.h"

namespace memfs {

VnodeDir::VnodeDir(Memfs& memfs) : Vnode(memfs), memfs_(memfs) {
  link_count_ = 1;  // Implied '.'
}

VnodeDir::~VnodeDir() = default;

fuchsia_io::NodeProtocolKinds VnodeDir::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kDirectory;
}

void VnodeDir::Notify(std::string_view name, fuchsia_io::wire::WatchEvent event) {
  watcher_.Notify(name, event);
}

zx_status_t VnodeDir::WatchDir(fs::FuchsiaVfs* vfs, fuchsia_io::wire::WatchMask mask,
                               uint32_t options,
                               fidl::ServerEnd<fuchsia_io::DirectoryWatcher> watcher) {
  return watcher_.WatchDir(vfs, this, mask, options, std::move(watcher));
}

zx_status_t VnodeDir::GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) {
  return ZX_ERR_ACCESS_DENIED;
}

zx_status_t VnodeDir::Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out) {
  if (!IsDirectory()) {
    return ZX_ERR_NOT_FOUND;
  }
  Dnode* dn = dnode_->Lookup(name);
  if (dn == nullptr) {
    return ZX_ERR_NOT_FOUND;
  }
  *out = dn->AcquireVnode();
  return ZX_OK;
}

zx::result<fs::VnodeAttributes> VnodeDir::GetAttributes() const {
  return zx::ok(fs::VnodeAttributes{
      .id = ino_,
      .content_size = 0,
      .storage_size = 0,
      .link_count = link_count_,
      .creation_time = create_time_,
      .modification_time = modify_time_,
      .mode = mode_,
      .uid = uid_,
      .gid = gid_,
      .rdev = rdev_,
  });
}

zx_status_t VnodeDir::Readdir(fs::VdirCookie* cookie, void* dirents, size_t len,
                              size_t* out_actual) {
  fs::DirentFiller df(dirents, len);
  if (!IsDirectory()) {
    // This WAS a directory, but it has been deleted.
    *out_actual = 0;
    return ZX_OK;
  }
  zx_status_t status = dnode_->Readdir(df, cookie);
  *out_actual = df.BytesFilled();
  return status;
}

zx::result<fbl::RefPtr<fs::Vnode>> VnodeDir::Create(std::string_view name, fs::CreationType type) {
  if (zx_status_t status = CanCreate(name); status != ZX_OK) {
    return zx::error(status);
  }

  fbl::RefPtr<memfs::Vnode> vn;
  bool is_dir = false;
  switch (type) {
    case fs::CreationType::kDirectory: {
      vn = fbl::MakeRefCounted<memfs::VnodeDir>(memfs_);
      is_dir = true;
      break;
    }
    case fs::CreationType::kFile: {
      vn = fbl::MakeRefCounted<VnodeFile>(memfs_);
      break;
    }
  }

  if (zx_status_t status = AttachVnode(vn, name, is_dir); status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(vn));
}

zx_status_t VnodeDir::Unlink(std::string_view name, bool must_be_dir) {
  if (!IsDirectory()) {
    // Calling unlink from unlinked, empty directory.
    return ZX_ERR_BAD_STATE;
  }
  Dnode* dn = dnode_->Lookup(name);
  if (dn == nullptr) {
    return ZX_ERR_NOT_FOUND;
  }
  if (!dn->IsDirectory() && must_be_dir) {
    // Path ending in "/" was requested, implying that the dnode must be a directory
    return ZX_ERR_NOT_DIR;
  }
  if (zx_status_t status = dn->CanUnlink(); status != ZX_OK) {
    return status;
  }

  dn->Detach();
  return ZX_OK;
}

zx_status_t VnodeDir::Rename(fbl::RefPtr<fs::Vnode> _newdir, std::string_view oldname,
                             std::string_view newname, bool src_must_be_dir, bool dst_must_be_dir) {
  auto newdir = fbl::RefPtr<Vnode>::Downcast(std::move(_newdir));

  if (!IsDirectory() || !newdir->IsDirectory()) {
    // Not linked into the directory hierarchy.
    return ZX_ERR_NOT_FOUND;
  }

  Dnode* olddn = dnode_->Lookup(oldname);
  // The source must exist
  if (olddn == nullptr) {
    return ZX_ERR_NOT_FOUND;
  }

  if (!olddn->IsDirectory() && (src_must_be_dir || dst_must_be_dir)) {
    return ZX_ERR_NOT_DIR;
  }
  if ((newdir->ino() == ino_) && (oldname == newname)) {
    // Renaming a file or directory to itself?
    // Shortcut success case
    return ZX_OK;
  }

  // Verify that the destination is not a subdirectory of the source (if
  // both are directories).
  if (olddn->IsSubdirectory(newdir->dnode_)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Allocate the new name for the dnode, either by
  // (1) Stealing it from the previous dnode, if it exists
  // (2) Allocating a new name, if creating a new name.
  std::string name;
  // The destination may or may not exist
  Dnode* target_dnode = newdir->dnode_->Lookup(newname);
  if (target_dnode != nullptr) {
    // The target exists. Validate and unlink it.
    if (olddn == target_dnode) {
      // Cannot rename node to itself
      return ZX_ERR_INVALID_ARGS;
    }
    if (olddn->IsDirectory() != target_dnode->IsDirectory()) {
      // Cannot rename files to directories (and vice versa)
      return olddn->IsDirectory() ? ZX_ERR_NOT_DIR : ZX_ERR_NOT_FILE;
    }
    if (zx_status_t status = target_dnode->CanUnlink(); status != ZX_OK) {
      return status;
    }
    name = target_dnode->TakeName();
    target_dnode->Detach();
  } else {
    name = newname;
  }

  // NOTE:
  //
  // Validation ends here, and modifications begin. Rename should not fail
  // beyond this point.

  std::unique_ptr<Dnode> moved_node = olddn->RemoveFromParent();
  olddn->PutName(std::move(name));
  Dnode::AddChild(newdir->dnode_, std::move(moved_node));
  return ZX_OK;
}

zx_status_t VnodeDir::Link(std::string_view name, fbl::RefPtr<fs::Vnode> target) {
  auto vn = fbl::RefPtr<Vnode>::Downcast(std::move(target));

  if (!IsDirectory()) {
    // Empty, unlinked parent
    return ZX_ERR_BAD_STATE;
  }

  if (vn->IsDirectory()) {
    // The target must not be a directory
    return ZX_ERR_NOT_FILE;
  }

  if (dnode_->Lookup(name) != nullptr) {
    // The destination should not exist
    return ZX_ERR_ALREADY_EXISTS;
  }

  // Make a new dnode for the new name, attach the target vnode to it
  auto target_dnone = Dnode::Create(std::string(name), vn);
  if (target_dnone.is_error()) {
    return target_dnone.status_value();
  }

  // Attach the new dnode to its parent
  Dnode::AddChild(dnode_, std::move(target_dnone).value());

  return ZX_OK;
}

zx_status_t VnodeDir::CreateFromVmo(std::string_view name, zx_handle_t vmo, zx_off_t off,
                                    zx_off_t len) {
  if (zx_status_t status = CanCreate(name); status != ZX_OK) {
    return status;
  }

  std::lock_guard lock(mutex_);

  auto vn = fbl::MakeRefCounted<VnodeVmo>(memfs_, vmo, off, len);
  if (zx_status_t status = AttachVnode(vn, name, false); status != ZX_OK) {
    return status;
  }

  return ZX_OK;
}

zx_status_t VnodeDir::CanCreate(std::string_view name) const {
  if (!IsDirectory()) {
    return ZX_ERR_BAD_STATE;
  }
  if (dnode_->Lookup(name) != nullptr) {
    return ZX_ERR_ALREADY_EXISTS;
  }
  return ZX_OK;
}

zx_status_t VnodeDir::AttachVnode(const fbl::RefPtr<Vnode>& vn, std::string_view name, bool isdir) {
  zx::result<std::unique_ptr<Dnode>> dn = Dnode::Create(std::string(name), vn);
  if (dn.is_error()) {
    return dn.status_value();
  }

  // Identify that the vnode is a directory (vn->dnode_ != nullptr) so that adding a child will also
  // increment the parent link_count (after all, directories contain a ".." entry, which is a link
  // to their parent).
  if (isdir) {
    vn->dnode_ = dn.value().get();
  }

  // Parent takes first reference.
  Dnode::AddChild(dnode_, std::move(dn).value());
  return ZX_OK;
}

}  // namespace memfs
