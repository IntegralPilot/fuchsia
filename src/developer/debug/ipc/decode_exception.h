// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_IPC_DECODE_EXCEPTION_H_
#define SRC_DEVELOPER_DEBUG_IPC_DECODE_EXCEPTION_H_

#include <lib/fit/function.h>
#include <stdint.h>

#include <optional>

#include "src/developer/debug/ipc/records.h"

namespace debug_ipc {

// Most exceptions can be convered just from the Zircon exception. But some require looking at the
// debug registers to disambiguate. Since getting the debug registers is uncommon, this API takes a
// callback that will retrieve them if needed.

struct X64DebugRegs {
  uint64_t dr0 = 0;
  uint64_t dr1 = 0;
  uint64_t dr2 = 0;
  uint64_t dr3 = 0;
  uint64_t dr6 = 0;
  uint64_t dr7 = 0;
};

ExceptionType DecodeX64Exception(uint32_t code,
                                 fit::function<std::optional<X64DebugRegs>()> fetch_debug_regs);
ExceptionType DecodeArm64Exception(uint32_t code,
                                   fit::function<std::optional<uint32_t>()> fetch_esr);
ExceptionType DecodeRiscv64Exception(uint32_t code);

}  // namespace debug_ipc

#endif  // SRC_DEVELOPER_DEBUG_IPC_DECODE_EXCEPTION_H_
