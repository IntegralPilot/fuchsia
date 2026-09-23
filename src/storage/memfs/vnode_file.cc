// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/memfs/vnode_file.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/zx/result.h>
#include <lib/zx/stream.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <mutex>
#include <utility>

#include <fbl/algorithm.h>
#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/shared_mutex.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode.h"

namespace memfs {

VnodeFile::VnodeFile(Memfs& memfs) : Vnode(memfs), memfs_(memfs) {}

VnodeFile::~VnodeFile() {
  fbl::RefPtr<fs::Vnode> file = FreePagedVmo();
  // FreePagedVmo is being called from the destructor so PagedVnode shouldn't have been holding onto
  // reference at this point.
  ZX_DEBUG_ASSERT(file == nullptr);
}

fuchsia_io::NodeProtocolKinds VnodeFile::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kFile;
}

zx::result<zx::stream> VnodeFile::CreateStream(uint32_t stream_options) {
  std::lock_guard lock(mutex_);
  if (zx_status_t status = CreateBackingStoreIfNeeded(); status != ZX_OK) {
    return zx::error(status);
  }
  zx::stream stream;
  if (zx_status_t status = zx::stream::create(stream_options, paged_vmo(), 0u, &stream);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(stream));
}

zx_status_t VnodeFile::GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) {
  std::lock_guard lock(mutex_);
  if (zx_status_t status = CreateBackingStoreIfNeeded(); status != ZX_OK) {
    return status;
  }
  size_t content_size = GetContentSize();
  // Let clients map and set the names of their VMOs.
  zx_rights_t rights = ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY;
  rights |= (flags & fuchsia_io::wire::VmoFlags::kRead) ? ZX_RIGHT_READ : 0;
  rights |= (flags & fuchsia_io::wire::VmoFlags::kWrite) ? ZX_RIGHT_WRITE : 0;
  rights |= (flags & fuchsia_io::wire::VmoFlags::kExecute) ? ZX_RIGHT_EXECUTE : 0;
  zx::vmo result;
  if (flags & fuchsia_io::wire::VmoFlags::kPrivateClone) {
    rights |= ZX_RIGHT_SET_PROPERTY;  // Only allow object_set_property on private VMO.
    if (zx_status_t status = paged_vmo().create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0,
                                                      content_size, &result);
        status != ZX_OK) {
      return status;
    }

    if (zx_status_t status = result.replace(rights, &result); status != ZX_OK) {
      return status;
    }
  } else {
    if (zx_status_t status = paged_vmo().duplicate(rights, &result); status != ZX_OK) {
      return status;
    }
  }

  *out_vmo = std::move(result);
  return ZX_OK;
}

zx::result<fs::VnodeAttributes> VnodeFile::GetAttributes() const {
  uint64_t content_size = 0;
  {
    fs::SharedLock lock(mutex_);
    if (paged_vmo().is_valid()) {
      content_size = GetContentSize();
      UpdateModifiedIfVmoChanged();
    }
  }

  return zx::ok(fs::VnodeAttributes{
      .id = ino_,
      .content_size = content_size,
      .storage_size = fbl::round_up(content_size, GetPageSize()),
      .link_count = link_count_,
      .creation_time = create_time_,
      .modification_time = modify_time_,
      .mode = mode_,
      .uid = uid_,
      .gid = gid_,
      .rdev = rdev_,
  });
}

zx::result<> VnodeFile::UpdateAttributes(const fs::VnodeAttributesUpdate& attributes) {
  // Reset the vmo stats when mtime is explicitly set.
  if (attributes.modification_time) {
    fs::SharedLock lock(mutex_);
    if (paged_vmo().is_valid()) {
      auto vmo_stats = memfs_.QueryVmoStats(paged_vmo(), ZX_PAGER_RESET_VMO_STATS);
      if (vmo_stats.is_error()) {
        return vmo_stats.take_error();
      }
    }
  }
  return Vnode::UpdateAttributes(attributes);
}

zx_status_t VnodeFile::Truncate(size_t length) {
  std::lock_guard lock(mutex_);
  if (zx_status_t status = CreateBackingStoreIfNeeded(); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = paged_vmo().set_stream_size(length); status != ZX_OK) {
    return status;
  }

  UpdateModified();
  return ZX_OK;
}

zx_status_t VnodeFile::CreateBackingStoreIfNeeded() {
  // TODO(https://fxbug.dev/42067655): Use a fixed sized VMO.
  return EnsureCreatePagedVmo(0, ZX_VMO_UNBOUNDED).status_value();
}

uint64_t VnodeFile::GetContentSize() const {
  ZX_DEBUG_ASSERT(paged_vmo().is_valid());
  uint64_t content_size = 0u;
  zx_status_t status = paged_vmo().get_prop_content_size(&content_size);
  ZX_DEBUG_ASSERT(status == ZX_OK);
  return content_size;
}

zx_status_t VnodeFile::CloseNode() {
  fs::SharedLock lock(mutex_);
  UpdateModifiedIfVmoChanged();
  return ZX_OK;
}

void VnodeFile::Sync(SyncCallback closure) {
  closure(ZX_OK);
  fs::SharedLock lock(mutex_);
  UpdateModifiedIfVmoChanged();
}

void VnodeFile::UpdateModifiedIfVmoChanged() const {
  if (!paged_vmo().is_valid()) {
    return;
  }

  auto vmo_stats = memfs_.QueryVmoStats(paged_vmo(), ZX_PAGER_RESET_VMO_STATS);
  ZX_ASSERT(vmo_stats.is_ok());
  if (vmo_stats->modified == ZX_PAGER_VMO_STATS_MODIFIED) {
    UpdateModified();
  }
}

}  // namespace memfs
