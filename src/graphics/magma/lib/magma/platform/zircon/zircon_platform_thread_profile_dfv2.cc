// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_thread.h>
#include <lib/magma/util/short_macros.h>
#include <lib/scheduler/role.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

namespace magma {

bool PlatformThreadHelper::SetRole(void* device_handle, const std::string& role_name) {
  zx_status_t status = fuchsia_scheduler::SetRoleForThisThread(role_name);
  if (status != ZX_OK) {
    return DRETF(false, "Failed to set role, status: %s", zx_status_get_string(status));
  }
  return true;
}

bool PlatformThreadHelper::SetThreadRole(void* device_handle, thrd_t thread,
                                         const std::string& role_name) {
  const zx_handle_t thread_handle = thrd_get_zx_handle(thread);
  if (thread_handle == ZX_HANDLE_INVALID) {
    return DRETF(false, "Invalid thread handle");
  }

  zx_status_t status =
      fuchsia_scheduler::SetRoleForThread(zx::unowned_thread(thread_handle), role_name);
  if (status != ZX_OK) {
    return DRETF(false, "Failed to set role, status: %s", zx_status_get_string(status));
  }
  return true;
}

}  // namespace magma
