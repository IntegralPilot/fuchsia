// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/vmo_file.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstddef>
#include <utility>

#include <fbl/algorithm.h>

#include "src/storage/lib/vfs/cpp/vfs_types.h"

namespace fio = fuchsia_io;

namespace fs {

VmoFile::VmoFile(zx::vmo vmo, size_t length, bool writable, DefaultSharingMode vmo_sharing)
    : vmo_(std::move(vmo)), length_(length), writable_(writable), vmo_sharing_(vmo_sharing) {
  ZX_ASSERT(vmo_.is_valid());
}

VmoFile::~VmoFile() = default;

fuchsia_io::NodeProtocolKinds VmoFile::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kFile;
}

bool VmoFile::ValidateRights(fuchsia_io::Rights rights) const {
  // Executable rights/VMOs are currently not supported, but may be added in the future.
  // If this is the case, we should further restrict the allowable set of rights such that
  // an executable VmoFile can only be opened as readable/executable and not writable.
  if (rights & fuchsia_io::Rights::kExecute) {
    return false;
  }
  if (!writable_ && rights & fuchsia_io::Rights::kWriteBytes) {
    return false;
  }
  return true;
}

zx::result<fs::VnodeAttributes> VmoFile::GetAttributes() const {
  return zx::ok(fs::VnodeAttributes{
      .content_size = length_,
      .storage_size = fbl::round_up(length_, zx_system_get_page_size()),
  });
}

zx_status_t VmoFile::Read(void* data, size_t length, size_t offset, size_t* out_actual) {
  if (length == 0u || offset >= length_) {
    *out_actual = 0u;
    return ZX_OK;
  }

  size_t remaining_length = length_ - offset;
  length = std::min(length, remaining_length);
  zx_status_t status = vmo_.read(data, offset, length);
  if (status != ZX_OK) {
    return status;
  }
  *out_actual = length;
  return ZX_OK;
}

zx_status_t VmoFile::Write(const void* data, size_t length, size_t offset, size_t* out_actual) {
  if (length == 0u) {
    *out_actual = 0u;
    return ZX_OK;
  }
  if (offset >= length_) {
    return ZX_ERR_NO_SPACE;
  }

  size_t remaining_length = length_ - offset;
  length = std::min(length, remaining_length);
  zx_status_t status = vmo_.write(data, offset, length);
  if (status == ZX_OK) {
    *out_actual = length;
  }
  return status;
}

zx_status_t VmoFile::GetVmo(fio::wire::VmoFlags flags, zx::vmo* out_vmo) {
  zx_rights_t rights = ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY;
  if (flags & fio::wire::VmoFlags::kRead) {
    rights |= ZX_RIGHT_READ;
  }
  if (flags & fio::wire::VmoFlags::kWrite) {
    rights |= ZX_RIGHT_WRITE | ZX_RIGHT_SET_PROPERTY;
  }
  if (flags & fio::wire::VmoFlags::kPrivateClone) {
    zx::vmo vmo;
    if (zx_status_t status =
            vmo_.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, length_, &vmo);
        status != ZX_OK) {
      return status;
    }
    return vmo.replace(rights, out_vmo);
  }
  if (flags & fio::wire::VmoFlags::kSharedBuffer) {
    return vmo_.duplicate(rights, out_vmo);
  }
  switch (vmo_sharing_) {
    case DefaultSharingMode::kNone:
      return ZX_ERR_NOT_SUPPORTED;
    case DefaultSharingMode::kDuplicate:
      // As size changes are currently untracked, we remove WRITE and SET_PROPERTY rights before
      // duplicating the VMO handle. If this restriction needs to be eased in the future, size
      // changes need to be tracked accordingly, or a fixed-size child slice should be provided.
      rights &= ~(ZX_RIGHT_WRITE | ZX_RIGHT_SET_PROPERTY);
      return vmo_.duplicate(rights, out_vmo);
    case DefaultSharingMode::kCloneCow: {
      zx::vmo vmo;
      if (zx_status_t status =
              vmo_.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, length_, &vmo);
          status != ZX_OK) {
        return status;
      }
      return vmo.replace(rights, out_vmo);
    }
  }
}

}  // namespace fs
