// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/sanitizer.h>

#include <string_view>

#include "debugdata.h"

int main(int argc, char* argv[]) {
  if (argc != 2) {
    return 1;
  }

  std::string_view mode = argv[1];
  bool fail;
  if (mode == "publish_data") {
    fail = false;
  } else if (mode == "publish_data_fail") {
    fail = true;
  } else {
    return 1;
  }

  zx::vmo vmo;
  ZX_ASSERT(zx::vmo::create(sizeof(kTestData), 0, &vmo) == ZX_OK);
  ZX_ASSERT(vmo.write(kTestData, 0, sizeof(kTestData)) == ZX_OK);
  ZX_ASSERT(vmo.set_property(ZX_PROP_NAME, kTestName, sizeof(kTestName)) == ZX_OK);
  zx_handle_t token = __sanitizer_publish_data(kTestName, vmo.release());
  ZX_ASSERT((token == ZX_HANDLE_INVALID) == fail);

  return 0;
}
