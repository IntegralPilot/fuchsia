// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>
#include <zircon/status.h>

#include "src/developer/debug/debug_agent/arch.h"
#include "src/developer/debug/shared/arch_x86.h"

// Our sysroot does not define this constant.
#ifndef TRAP_HWBKPT
#define TRAP_HWBKPT 4
#endif

namespace debug_agent {
namespace arch {

uint32_t GetHardwareBreakpointCount() {
  // TODO(brettW) implement this.
  return 4;
}

uint32_t GetHardwareWatchpointCount() {
  // TODO(brettW) implement this.
  return 4;
}

void SaveGeneralRegs(const PlatformGeneralRegisters& input,
                     std::vector<debug::RegisterValue>& out) {
  using debug::RegisterID;
  out.emplace_back(RegisterID::kX64_rax, static_cast<uint64_t>(input.rax));
  out.emplace_back(RegisterID::kX64_rbx, static_cast<uint64_t>(input.rbx));
  out.emplace_back(RegisterID::kX64_rcx, static_cast<uint64_t>(input.rcx));
  out.emplace_back(RegisterID::kX64_rdx, static_cast<uint64_t>(input.rdx));
  out.emplace_back(RegisterID::kX64_rsi, static_cast<uint64_t>(input.rsi));
  out.emplace_back(RegisterID::kX64_rdi, static_cast<uint64_t>(input.rdi));
  out.emplace_back(RegisterID::kX64_rbp, static_cast<uint64_t>(input.rbp));
  out.emplace_back(RegisterID::kX64_rsp, static_cast<uint64_t>(input.rsp));
  out.emplace_back(RegisterID::kX64_r8, static_cast<uint64_t>(input.r8));
  out.emplace_back(RegisterID::kX64_r9, static_cast<uint64_t>(input.r9));
  out.emplace_back(RegisterID::kX64_r10, static_cast<uint64_t>(input.r10));
  out.emplace_back(RegisterID::kX64_r11, static_cast<uint64_t>(input.r11));
  out.emplace_back(RegisterID::kX64_r12, static_cast<uint64_t>(input.r12));
  out.emplace_back(RegisterID::kX64_r13, static_cast<uint64_t>(input.r13));
  out.emplace_back(RegisterID::kX64_r14, static_cast<uint64_t>(input.r14));
  out.emplace_back(RegisterID::kX64_r15, static_cast<uint64_t>(input.r15));
  out.emplace_back(RegisterID::kX64_rip, static_cast<uint64_t>(input.rip));
  out.emplace_back(RegisterID::kX64_rflags, static_cast<uint64_t>(input.eflags));
  out.emplace_back(RegisterID::kX64_fsbase, static_cast<uint64_t>(input.fs_base));
  out.emplace_back(RegisterID::kX64_gsbase, static_cast<uint64_t>(input.gs_base));

  // The Linux user_regs_struct also contains "cs", "ds", "es", "fs", "gs", and "ss" which we don't
  // have enums for. They should be added when we have them.
}

debug_ipc::ExceptionType DecodeExceptionType(int signal, int sig_code) {
  // TODO(brettw) fill out different singal types. See bits/siginfo-consts.h
  switch (signal) {
    case SIGTRAP:
      switch (sig_code) {
        case SI_KERNEL:
          return debug_ipc::ExceptionType::kSoftwareBreakpoint;
        case TRAP_TRACE:
          return debug_ipc::ExceptionType::kSingleStep;
        case TRAP_BRKPT:
          // Single-stepping a syscall.
          // TODO(brettw) do we need something different here?
          return debug_ipc::ExceptionType::kSingleStep;
        case TRAP_HWBKPT:
          return debug_ipc::ExceptionType::kHardwareBreakpoint;
      }
      return debug_ipc::ExceptionType::kGeneral;
    default:
      return debug_ipc::ExceptionType::kUnknown;
  }
}

}  // namespace arch
}  // namespace debug_agent
