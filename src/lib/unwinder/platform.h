// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_PLATFORM_H_
#define SRC_LIB_UNWINDER_PLATFORM_H_

#if defined(__Fuchsia__)
#include "src/lib/unwinder/fuchsia.h"
#elif defined(__linux__)
#include "src/lib/unwinder/linux.h"
#else
#error Unwinder not supported on this platform.
#endif

namespace unwinder {

#if defined(__Fuchsia__)
using PlatformRegisters = zx_thread_state_general_regs_t;
#elif defined(__linux__)
using PlatformRegisters = struct user_regs_struct;
#else
#error Need PlatformRegisters for this platform.
#endif

Registers FromPlatformRegisters(const PlatformRegisters& regs) {
#if defined(__Fuchsia__)
  return FromFuchsiaRegisters(regs);
#elif defined(__linux__)
  return FromLinuxRegisters(regs);
#else
#error Need PlatformRegisters for this platform.
#endif
}

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_PLATFORM_H_
