// Copyright 2025 The Fuchsia Authors. All rights reserved.
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

const size_t RegisterState::kTlsStorageSize = sizeof(TlsStorage);

// The number of bytes needed to hold the FPU's state.
// RISC-V has 32 8-byte floating-point registers.
const uint16_t RegisterState::kFpuBufferSize = 32 * 16;

void RegisterState::set_tls(TlsStorage* tls_storage) {
  // Configure TLS storage.
  tls_ = tls_storage;
  tls()->tp = 0;
  state_.tp = reinterpret_cast<uintptr_t>(&tls()->tp);
}

void RegisterState::InitializeRegisters() {
  // Initialize all standard registers to 0.
  state_ = {0};
}

void RegisterState::InitializeFromThreadState(const zx_thread_state_general_regs_t& regs) {
  static_assert(sizeof(regs) <= sizeof(state_));
  memcpy(&state_, &regs, sizeof(regs));
}

uintptr_t RegisterState::pc() const { return state_.pc; }
uintptr_t RegisterState::syscall_number() const { return state_.a7; }
void RegisterState::set_syscall_number(uint64_t number) { state_.a7 = number; }
uintptr_t RegisterState::syscall_arg(unsigned index) const {
  ZX_ASSERT(index <= 5);
  switch (index) {
    case 0:
      return state_.a0;
    case 1:
      return state_.a1;
    case 2:
      return state_.a2;
    case 3:
      return state_.a3;
    case 4:
      return state_.a4;
    case 5:
      return state_.a5;
    default:
      return 0;
  }
}
void RegisterState::set_syscall_return(uint64_t value) { state_.a0 = value; }
void RegisterState::set_pc(uintptr_t pc) { state_.pc = pc; }
void RegisterState::set_sp(uintptr_t sp) { state_.sp = sp; }
void RegisterState::set_shadow_sp(uintptr_t sp) { state_.gp = sp; }
void RegisterState::set_arg_regs(uint64_t arg0, uint64_t arg1) {
  state_.a0 = arg0;
  state_.a1 = arg1;
}

// This is from RestrictedMode::ArchDump in zircon/kernel.
void RegisterState::PrintState(const zx_restricted_state_t& state) {
  RM_LOG(ERROR) << "PC: 0x" << std::hex << state.pc;
  RM_LOG(ERROR) << "RA: 0x" << std::hex << state.ra;
  RM_LOG(ERROR) << "SP: 0x" << std::hex << state.sp;
  RM_LOG(ERROR) << "GP: 0x" << std::hex << state.gp;
  RM_LOG(ERROR) << "TP: 0x" << std::hex << state.tp;
  RM_LOG(ERROR) << "T0: 0x" << std::hex << state.t0;
  RM_LOG(ERROR) << "T1: 0x" << std::hex << state.t1;
  RM_LOG(ERROR) << "T2: 0x" << std::hex << state.t2;
  RM_LOG(ERROR) << "S0: 0x" << std::hex << state.s0;
  RM_LOG(ERROR) << "S1: 0x" << std::hex << state.s1;
  RM_LOG(ERROR) << "A0: 0x" << std::hex << state.a0;
  RM_LOG(ERROR) << "A1: 0x" << std::hex << state.a1;
  RM_LOG(ERROR) << "A2: 0x" << std::hex << state.a2;
  RM_LOG(ERROR) << "A3: 0x" << std::hex << state.a3;
  RM_LOG(ERROR) << "A4: 0x" << std::hex << state.a4;
  RM_LOG(ERROR) << "A5: 0x" << std::hex << state.a5;
  RM_LOG(ERROR) << "A6: 0x" << std::hex << state.a6;
  RM_LOG(ERROR) << "A7: 0x" << std::hex << state.a7;
  RM_LOG(ERROR) << "S2: 0x" << std::hex << state.s2;
  RM_LOG(ERROR) << "S3: 0x" << std::hex << state.s3;
  RM_LOG(ERROR) << "S4: 0x" << std::hex << state.s4;
  RM_LOG(ERROR) << "S5: 0x" << std::hex << state.s5;
  RM_LOG(ERROR) << "S6: 0x" << std::hex << state.s6;
  RM_LOG(ERROR) << "S7: 0x" << std::hex << state.s7;
  RM_LOG(ERROR) << "S8: 0x" << std::hex << state.s8;
  RM_LOG(ERROR) << "S9: 0x" << std::hex << state.s9;
  RM_LOG(ERROR) << "S10: 0x" << std::hex << state.s10;
  RM_LOG(ERROR) << "S11: 0x" << std::hex << state.s11;
  RM_LOG(ERROR) << "T3: 0x" << std::hex << state.t3;
  RM_LOG(ERROR) << "T4: 0x" << std::hex << state.t4;
  RM_LOG(ERROR) << "T5: 0x" << std::hex << state.t5;
  RM_LOG(ERROR) << "T6: 0x" << std::hex << state.t6;
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
