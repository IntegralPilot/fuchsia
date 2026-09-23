// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/iob.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/iob.h>

namespace zx {

zx_status_t iob::create(uint64_t options, zx_iob_region_t* regions, uint32_t region_count,
                        iob* endpoint0, iob* endpoint1) {
  // Ensure aliasing of both out parameters to the same container
  // has a well-defined result, and does not leak.
  iob h0;
  iob h1;
  zx_status_t status = zx_iob_create(options, regions, region_count, h0.reset_and_get_address(),
                                     h1.reset_and_get_address());
  endpoint0->reset(h0.release());
  endpoint1->reset(h1.release());
  return status;
}

}  // namespace zx
