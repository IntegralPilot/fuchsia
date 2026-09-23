// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_trace.h>
#include <zircon/syscalls.h>

namespace magma {

// static
uint64_t PlatformTrace::GetCurrentTicks() { return zx_ticks_get_boot(); }

}  // namespace magma
