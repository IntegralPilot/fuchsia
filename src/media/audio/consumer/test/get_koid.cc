// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/consumer/test/get_koid.h"

#include <lib/syslog/cpp/macros.h>

namespace media::audio::tests {

zx_koid_t GetKoid(const zx::vmo& vmo) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(vmo.get(), ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  FX_CHECK(status == ZX_OK);
  return info.koid;
}

}  // namespace media::audio::tests
