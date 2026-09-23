// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MEMFS_MEMFS_H_
#define SRC_STORAGE_MEMFS_MEMFS_H_

#include <lib/async/dispatcher.h>
#include <lib/zx/event.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <string_view>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/paged_vfs.h"
#include "src/storage/memfs/dnode.h"

namespace memfs {

class VnodeDir;

// Returns the page size used by Memfs (this is just the system memory page size).
uint64_t GetPageSize();

class Memfs : public fs::PagedVfs {
 public:
  Memfs(const Memfs&) = delete;
  Memfs& operator=(const Memfs&) = delete;

  static zx::result<std::pair<std::unique_ptr<Memfs>, fbl::RefPtr<VnodeDir>>> Create(
      async_dispatcher_t* dispatcher, std::string_view fs_name);

  ~Memfs() override;

  // Creates a VnodeVmo under |parent| with |name| which is backed by |vmo|.
  // N.B. The VMO will not be taken into account when calculating
  // number of allocated pages in this Memfs.
  zx_status_t CreateFromVmo(VnodeDir* parent, std::string_view name, zx_handle_t vmo, zx_off_t off,
                            zx_off_t len);

  // fs::FuchsiaVfs override:
  zx::result<fs::FilesystemInfo> GetFilesystemInfo() override;

 private:
  explicit Memfs(async_dispatcher_t* dispatcher);

  // This event's koid is used as a unique identifier for this filesystem instance.
  zx::event fs_id_;

  // Since no directory contains the root, it is owned by the VFS object.
  std::unique_ptr<Dnode> root_;
};

}  // namespace memfs

#endif  // SRC_STORAGE_MEMFS_MEMFS_H_
