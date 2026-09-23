// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/thread.h>
#include <stdint.h>

#include <limits>

#include <android-base/threads.h>

namespace android {
namespace base {

uint64_t GetThreadId() {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx::thread::self()->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : std::numeric_limits<uint64_t>::max();
}

}  // namespace base
}  // namespace android
