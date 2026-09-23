// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/devfs/builtin_devices.h"

#include <sys/stat.h>

#include "src/storage/lib/vfs/cpp/vfs_types.h"

namespace driver_manager {

zx_status_t BuiltinDevVnode::Read(void* data, size_t len, size_t off, size_t* out_actual) {
  // /dev/null implementation.
  if (null_) {
    *out_actual = 0;
    return ZX_OK;
  }
  // /dev/zero implementation.
  memset(data, 0, len);
  *out_actual = len;
  return ZX_OK;
}

zx_status_t BuiltinDevVnode::Write(const void* data, size_t len, size_t off, size_t* out_actual) {
  if (null_) {
    *out_actual = len;
    return ZX_OK;
  }
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t BuiltinDevVnode::Truncate(size_t len) { return ZX_OK; }

zx::result<fs::VnodeAttributes> BuiltinDevVnode::GetAttributes() const {
  return zx::ok(fs::VnodeAttributes{
      .mode = S_IFCHR | S_IRUSR | S_IWUSR,
  });
}

fuchsia_io::NodeProtocolKinds BuiltinDevVnode::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kFile;
}

}  // namespace driver_manager
