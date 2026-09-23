// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/counter.h>
#include <zircon/syscalls.h>

namespace zx {

zx_status_t counter::create(uint32_t options, counter* result) {
  return zx_counter_create(options, result->reset_and_get_address());
}

}  // namespace zx
