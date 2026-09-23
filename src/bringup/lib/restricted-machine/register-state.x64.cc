// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>
#include <zircon/features.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <bringup/lib/restricted-machine/internal/arch-helpers.h>
#include <bringup/lib/restricted-machine/internal/common.h>
#include <bringup/lib/restricted-machine/machine-type.h>
#include <bringup/lib/restricted-machine/register-state.h>
#include <bringup/lib/restricted-machine/tls-storage.h>

namespace restricted_machine {

// The number of bytes needed to hold the FPU's state.
// x86 has 8 10-byte registers, followed by 16 16-byte registers.
const uint16_t RegisterState::kFpuBufferSize = (8 * 10) + (16 * 16);

const size_t RegisterState::kTlsStorageSize = sizeof(TlsStorage);
void RegisterState::InitializeRegisters() { state_ = {0}; }

void RegisterState::InitializeFromThreadState(const zx_thread_state_general_regs_t& regs) {
  state_.flags = regs.rflags;
  state_.rax = regs.rax;
  state_.rbx = regs.rbx;
  state_.rcx = regs.rcx;
  state_.rdx = regs.rdx;
  state_.rsi = regs.rsi;
  state_.rdi = regs.rdi;
  state_.rbp = regs.rbp;
  state_.rsp = regs.rsp;
  state_.r8 = regs.r8;
  state_.r9 = regs.r9;
  state_.r10 = regs.r10;
  state_.r11 = regs.r11;
  state_.r12 = regs.r12;
  state_.r13 = regs.r13;
  state_.r14 = regs.r14;
  state_.r15 = regs.r15;
  state_.fs_base = regs.fs_base;
  state_.gs_base = regs.gs_base;
  state_.ip = regs.rip;
}

uintptr_t RegisterState::pc() const { return state_.ip; }
uint64_t RegisterState::syscall_number() const { return state_.rax; }
void RegisterState::set_syscall_number(uint64_t number) { state_.rax = number; }
void RegisterState::set_syscall_return(uint64_t value) { state_.rax = value; }
uintptr_t RegisterState::syscall_arg(unsigned index) const {
  ZX_ASSERT(index <= 5);
  switch (index) {
    case 0:
      return state_.rdi;
    case 1:
      return state_.rsi;
    case 2:
      return state_.rdx;
    case 3:
      return state_.r10;
    case 4:
      return state_.r8;
    case 5:
      return state_.r9;
    default:
      return 0;
  }
}
void RegisterState::set_pc(uintptr_t pc) { state_.ip = pc; }
void RegisterState::set_sp(uintptr_t sp) { state_.rsp = sp; }
void RegisterState::set_shadow_sp(uintptr_t sp) {}
void RegisterState::set_arg_regs(uint64_t arg0, uint64_t arg1) {
  state_.rdi = arg0;
  state_.rsi = arg1;
}

void RegisterState::set_tls(TlsStorage* tls_storage) {
  tls_ = tls_storage;
  // Configure TLS storage
  tls()->fs_val = 0;
  tls()->gs_val = 0;
  state_.fs_base = reinterpret_cast<uintptr_t>(&tls()->fs_val);
  state_.gs_base = reinterpret_cast<uintptr_t>(&tls()->gs_val);
}

void RegisterState::PrintState(const zx_restricted_state_t& state) {
  RM_LOG(ERROR) << " RIP: 0x" << std::hex << state.ip << "  FL: 0x" << std::hex << state.flags;
  RM_LOG(ERROR) << " RAX: 0x" << std::hex << state.rax << " RBX: 0x" << std::hex << state.rbx
                << " RCX: 0x" << std::hex << state.rcx << " RDX: 0x" << std::hex << state.rdx;
  RM_LOG(ERROR) << " RSI: 0x" << std::hex << state.rsi << " RDI: 0x" << std::hex << state.rdi
                << " RBP: 0x" << std::hex << state.rbp << " RSP: 0x" << std::hex << state.rsp;
  RM_LOG(ERROR) << "  R8: 0x" << std::hex << state.r8 << "  R9: 0x" << std::hex << state.r9
                << " R10: 0x" << std::hex << state.r10 << " R11: 0x" << std::hex << state.r11;
  RM_LOG(ERROR) << " R12: 0x" << std::hex << state.r12 << " R13: 0x" << std::hex << state.r13
                << " R14: 0x" << std::hex << state.r14 << " R15: 0x" << std::hex << state.r15;
  RM_LOG(ERROR) << "fs base 0x" << std::hex << state.fs_base << " gs base 0x" << std::hex
                << state.gs_base;
}

void RegisterState::PrintExceptionState(const zx_restricted_exception_t& exc) {
  PrintExceptionReport(exc.exception);
  PrintState(exc.state);
}

void RegisterState::PrintExceptionReport(const zx_exception_report_t& report) {
  RM_LOG(ERROR) << "type: 0x" << std::hex << report.header.type;
  RM_LOG(ERROR) << "synth_code: 0x" << std::hex << report.context.synth_code;
  RM_LOG(ERROR) << "synth_data: 0x" << std::hex << report.context.synth_data;
}

// Map the types to machines.
std::unique_ptr<RegisterState> RegisterStateFactory::Create(const MachineType& machine) {
  assert(machine == MachineType::kNative);
  return std::make_unique<RegisterState>();
}

}  // namespace restricted_machine
