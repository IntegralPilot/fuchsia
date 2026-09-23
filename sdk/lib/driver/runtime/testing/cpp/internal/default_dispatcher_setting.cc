// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/runtime/testing/cpp/internal/default_dispatcher_setting.h>
#include <lib/fdf/testing.h>
#include <zircon/status.h>

namespace fdf_internal {

DefaultDispatcherSetting::DefaultDispatcherSetting(fdf_dispatcher_t* dispatcher) {
  zx_status_t status = fdf_testing_set_default_dispatcher(dispatcher);
  ZX_ASSERT_MSG(ZX_OK == status, "Failed to set default dispatcher setting: %s",
                zx_status_get_string(status));
}

DefaultDispatcherSetting::~DefaultDispatcherSetting() {
  zx_status_t status = fdf_testing_set_default_dispatcher(nullptr);
  ZX_ASSERT_MSG(ZX_OK == status, "Failed to remove default dispatcher setting: %s",
                zx_status_get_string(status));
}

}  // namespace fdf_internal
