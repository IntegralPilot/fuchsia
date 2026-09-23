// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_SCUDO_GWP_ASAN_INFO_H_
#define LIB_C_SCUDO_GWP_ASAN_INFO_H_

#ifndef __ASSEMBLER__

#include "gwp_asan/common.h"
#include "gwp_asan/crash_handler.h"

namespace gwp_asan {

struct LibcGwpAsanInfo {
  const gwp_asan::AllocatorState* state = nullptr;
  const gwp_asan::AllocationMetadata* metadata = nullptr;
};

}  // namespace gwp_asan

// NOLINTNEXTLINE(bugprone-reserved-identifier)
extern gwp_asan::LibcGwpAsanInfo __libc_gwp_asan_info;

#endif

#define GWP_ASAN_NOTE_TYPE 0x4153414e  // "ASAN"

#endif  // LIB_C_SCUDO_GWP_ASAN_INFO_H_
