// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/channel.h>
#include <zircon/syscalls.h>

namespace zx {

// Constructs a tick object for the current tick counter in the system in the monotonic timeline.
template <>
basic_ticks<ZX_CLOCK_MONOTONIC> basic_ticks<ZX_CLOCK_MONOTONIC>::now() ZX_AVAILABLE_SINCE(7) {
  return basic_ticks(zx_ticks_get());
}

// Constructs a tick object for the current tick counter in the system in the boot timeline.
template <>
basic_ticks<ZX_CLOCK_BOOT> basic_ticks<ZX_CLOCK_BOOT>::now() ZX_AVAILABLE_SINCE(25) {
  return basic_ticks(zx_ticks_get_boot());
}

}  // namespace zx
