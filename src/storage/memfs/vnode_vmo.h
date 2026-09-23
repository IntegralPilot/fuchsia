// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MEMFS_VNODE_VMO_H_
#define SRC_STORAGE_MEMFS_VNODE_VMO_H_

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <cstddef>

#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode.h"

namespace memfs {

class VnodeVmo final : public Vnode {
 public:
  VnodeVmo(Memfs& memfs, zx_handle_t vmo, zx_off_t offset, zx_off_t length);
  ~VnodeVmo() override;

  fuchsia_io::NodeProtocolKinds GetProtocols() const final;
  bool ValidateRights(fuchsia_io::Rights rights) const final;

 private:
  zx_status_t Read(void* data, size_t len, size_t off, size_t* out_actual) final;
  zx::result<fs::VnodeAttributes> GetAttributes() const final;
  zx_status_t GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) final;
  zx_status_t MakeLocalClone();

  zx_handle_t vmo_ = ZX_HANDLE_INVALID;
  zx_off_t offset_ = 0;
  zx_off_t length_ = 0;
  bool executable_ = false;
  bool have_local_clone_ = false;
};

}  // namespace memfs

#endif  // SRC_STORAGE_MEMFS_VNODE_VMO_H_
