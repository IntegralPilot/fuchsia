// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The sole purpose of this is to publish a vmo with known contents to the provided
// '/svc' handle through the __sanitizer_publish_data.

#include <lib/zx/vmo.h>
#include <stdio.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/limits.h>
#include <zircon/sanitizer.h>

#include "published-data.h"

int main(int argc, const char**) {
  zx::vmo data;

  ZX_ASSERT(zx::vmo::create(zx_system_get_page_size(), 0, &data) == ZX_OK);
  ZX_ASSERT(data.write(kVmoContents.data(), 0, kVmoContents.size()) == ZX_OK);

  __sanitizer_publish_data(kPublisherSinkName.data(), data.release());
  return 0;
}
