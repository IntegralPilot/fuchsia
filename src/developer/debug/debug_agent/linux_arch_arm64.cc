// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include "src/developer/debug/debug_agent/arch.h"

namespace debug_agent {
namespace arch {

uint32_t GetHardwareBreakpointCount() {
  // TODO(brettW) implement this.
  FX_NOTREACHED();
  return 4;
}

uint32_t GetHardwareWatchpointCount() {
  // TODO(brettW) implement this.
  FX_NOTREACHED();
  return 4;
}

void SaveGeneralRegs(const PlatformGeneralRegisters& input,
                     std::vector<debug::RegisterValue>& out) {
  FX_NOTREACHED();
}

debug_ipc::ExceptionType DecodeExceptionType(int signal, int sig_code) {
  // TODO(brettw) fill out different singal types. See bits/siginfo-consts.h
  FX_NOTREACHED();
  return debug_ipc::ExceptionType::kUnknown;
}

}  // namespace arch
}  // namespace debug_agent
