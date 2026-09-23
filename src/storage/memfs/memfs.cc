// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/memfs/memfs.h"

#include <fidl/fuchsia.fs/cpp/common_types.h>
#include <lib/async/dispatcher.h>
#include <lib/zx/event.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <utility>

#include <fbl/ref_ptr.h>
#include <safemath/safe_conversions.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/paged_vfs.h"
#include "src/storage/memfs/dnode.h"
#include "src/storage/memfs/vnode_dir.h"

namespace memfs {

size_t GetPageSize() {
  static const size_t kPageSize = static_cast<size_t>(zx_system_get_page_size());
  return kPageSize;
}

zx::result<fs::FilesystemInfo> Memfs::GetFilesystemInfo() {
  fs::FilesystemInfo info;

  info.block_size = safemath::checked_cast<uint32_t>(GetPageSize());
  info.max_filename_size = kDnodeNameMax;
  info.fs_type = fuchsia_fs::VfsType::kMemfs;
  info.SetFsId(fs_id_);

  // memfs vends writable duplicates of its underlying VMOs to its clients which makes accounting
  // difficult, so used_bytes is reported as 0.  We used to return UINT64_MAX for total_bytes, but
  // that caused issues with clients which attempted to do arithmetic on the value, so we now return
  // the available system memory, so some reasonable value is returned.
  info.total_bytes = zx_system_get_physmem();
  info.used_bytes = 0;
  info.total_nodes = UINT64_MAX;
  uint64_t deleted_ino_count = Vnode::GetDeletedInoCounter();
  uint64_t ino_count = Vnode::GetInoCounter();
  ZX_DEBUG_ASSERT(ino_count >= deleted_ino_count);
  info.used_nodes = ino_count - deleted_ino_count;
  info.name = "memfs";

  return zx::ok(info);
}

zx::result<std::pair<std::unique_ptr<Memfs>, fbl::RefPtr<VnodeDir>>> Memfs::Create(
    async_dispatcher_t* dispatcher, std::string_view fs_name) {
  std::unique_ptr<Memfs> fs(new Memfs(dispatcher));

  fbl::RefPtr<VnodeDir> root = fbl::MakeRefCounted<VnodeDir>(*fs);
  zx::result<std::unique_ptr<Dnode>> dn = Dnode::Create(std::string(fs_name), root);
  if (dn.is_error()) {
    return dn.take_error();
  }
  root->dnode_ = dn.value().get();
  fs->root_ = std::move(dn).value();

  if (zx::result<> result = fs->Init(); result.is_error()) {
    return result.take_error();
  }

  if (zx_status_t status = zx::event::create(0, &fs->fs_id_); status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::make_pair(std::move(fs), std::move(root)));
}

Memfs::Memfs(async_dispatcher_t* dispatcher) : fs::PagedVfs(dispatcher) {}

Memfs::~Memfs() { TearDown(); }

zx_status_t Memfs::CreateFromVmo(VnodeDir* parent, std::string_view name, zx_handle_t vmo,
                                 zx_off_t off, zx_off_t len) {
  std::lock_guard<std::mutex> lock(vfs_lock_);
  return parent->CreateFromVmo(name, vmo, off, len);
}

}  // namespace memfs
