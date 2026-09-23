// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MEMFS_VNODE_FILE_H_
#define SRC_STORAGE_MEMFS_VNODE_FILE_H_

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/zx/result.h>
#include <lib/zx/stream.h>
#include <lib/zx/vmo.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>

#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode.h"

namespace memfs {

class VnodeFile final : public Vnode {
 public:
  explicit VnodeFile(Memfs& memfs);
  ~VnodeFile() final;

  fuchsia_io::NodeProtocolKinds GetProtocols() const final;

  zx::result<zx::stream> CreateStream(uint32_t stream_options) final;

  zx_status_t Truncate(size_t len) final;
  zx::result<fs::VnodeAttributes> GetAttributes() const final;
  zx::result<> UpdateAttributes(const fs::VnodeAttributesUpdate& attributes) final;
  zx_status_t GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) final;
  zx_status_t CloseNode() final;
  void Sync(SyncCallback closure) final;

 private:
  zx_status_t CreateBackingStoreIfNeeded() __TA_REQUIRES(mutex_);

  // Returns the content size of |paged_vmo()|. Requires |paged_vmo()| to be valid.
  uint64_t GetContentSize() const __TA_REQUIRES_SHARED(mutex_);

  // Checks to see if the contents of this file were modified since the last time this method was
  // called. If the file was modified then the mtime is updated.
  void UpdateModifiedIfVmoChanged() const __TA_REQUIRES_SHARED(mutex_);

  [[maybe_unused]] Memfs& memfs_;
};

}  // namespace memfs

#endif  // SRC_STORAGE_MEMFS_VNODE_FILE_H_
