// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/vmo.h"

#include <lib/syslog/cpp/macros.h>

namespace forensics {

zx::result<std::string> StringFromVmo(const zx::unowned_vmo& vmo) {
  uint64_t size;
  if (const zx_status_t status = vmo->get_stream_size(&size); status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "Failed to get VMO stream size";
    return zx::error(status);
  }

  if (size == 0) {
    return zx::ok("");
  }

  std::string contents(size, '\0');
  if (const zx_status_t status = vmo->read(contents.data(), /*offset=*/0, size); status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "Failed to read VMO";
    return zx::error(status);
  }

  return zx::ok(std::move(contents));
}

zx::result<std::string> StringFromVmo(const zx::vmo& vmo) { return StringFromVmo(vmo.borrow()); }

zx::result<zx::vmo> VmoFromString(std::string_view string) {
  zx::vmo vmo;
  if (const zx_status_t status = zx::vmo::create(string.size(), /*options=*/0, &vmo);
      status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "Failed to create VMO of size " << string.size();
    return zx::error(status);
  }

  if (string.empty()) {
    return zx::ok(std::move(vmo));
  }

  if (const zx_status_t status = vmo.write(string.data(), /*offset=*/0, string.size());
      status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "Failed to write to VMO";
    return zx::error(status);
  }

  return zx::ok(std::move(vmo));
}

}  // namespace forensics
