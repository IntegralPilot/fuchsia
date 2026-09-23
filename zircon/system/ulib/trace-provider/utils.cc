// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "utils.h"

#include <lib/zx/process.h>
#include <stdio.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

namespace trace {
namespace internal {

zx_koid_t GetPid() {
  auto self = zx_process_self();
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(self, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    return ZX_KOID_INVALID;
  }
  return info.koid;
}

zx::result<std::string> GetProcessName() {
  auto self = zx::process::self();
  char name[ZX_MAX_NAME_LEN];
  auto status = self->get_property(ZX_PROP_NAME, name, sizeof(name));
  if (status != ZX_OK) {
    fprintf(stderr, "TraceProvider: error getting process name: status=%d(%s)\n", status,
            zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok(name);
}

}  // namespace internal
}  // namespace trace
