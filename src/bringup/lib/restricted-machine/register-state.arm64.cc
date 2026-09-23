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

const size_t RegisterState::kTlsStorageSize = sizeof(TlsStorage);

// The number of bytes needed to hold the FPU's state.
// ARM has 32 16-byte floating-point registers.
const uint16_t RegisterState::kFpuBufferSize = 32 * 16;

void RegisterState::set_tls(TlsStorage* tls_storage) {
  // Initialize a new thread local storage for the restricted mode routine.
  tls_ = tls_storage;
  tls()->tpidr = 0;
  state_.tpidr_el0 = reinterpret_cast<uintptr_t>(&tls()->tpidr);
}

void RegisterState::InitializeRegisters() {
  // Initialize all standard registers to 0 except where otherwise provided.
  //
  // TLS is a bit of a throwback to the original class design, so it may be factored
  // out in the future.
  state_ = {
      .x = {0},
      .sp = 0x0,
      .tpidr_el0 = 0,
      .cpsr = 0,
  };
}

void RegisterState::InitializeFromThreadState(const zx_thread_state_general_regs_t& regs) {
  static_assert(sizeof(regs.r) <= sizeof(state_.x));
  memcpy(state_.x, regs.r, sizeof(regs.r));
  state_.x[30] = regs.lr;
  state_.pc = regs.pc;
  state_.tpidr_el0 = regs.tpidr;
  state_.sp = regs.sp;
  state_.cpsr = static_cast<uint32_t>(regs.cpsr);
}

uintptr_t RegisterState::pc() const { return state_.pc; }
uint64_t RegisterState::syscall_number() const { return state_.x[8]; }
void RegisterState::set_syscall_number(uint64_t number) { state_.x[8] = number; }
uintptr_t RegisterState::syscall_arg(unsigned index) const {
  ZX_ASSERT(index <= 5);
  return state_.x[index];
}
void RegisterState::set_pc(uintptr_t pc) { state_.pc = pc; }
void RegisterState::set_sp(uintptr_t sp) { state_.sp = sp; }
void RegisterState::set_shadow_sp(uintptr_t sp) { state_.x[18] = sp; }
void RegisterState::set_syscall_return(uint64_t value) { state_.x[0] = value; }
void RegisterState::set_arg_regs(uint64_t arg0, uint64_t arg1) {
  state_.x[0] = arg0;
  state_.x[1] = arg1;
}
// This is from RestrictedMode::ArchDump in zircon/kernel.
void RegisterState::PrintState(const zx_restricted_state_t& state) {
  // Helper for log analysis for unexpected exceptions.
  for (size_t i = 0; i < std::size(state.x); i++) {
    RM_LOG(ERROR) << "R" << i << ": 0x" << std::hex << state.x[i];
  }
  RM_LOG(ERROR) << "CPSR: 0x" << std::hex << state.cpsr;
  RM_LOG(ERROR) << "PC: 0x" << std::hex << state.pc;
  RM_LOG(ERROR) << "SP: 0x" << std::hex << state.sp;
  RM_LOG(ERROR) << "TPIDR_EL0: 0x" << std::hex << state.tpidr_el0;
}

void RegisterState::PrintExceptionState(const zx_restricted_exception_t& exc) {
  PrintExceptionReport(exc.exception);
  PrintState(exc.state);
}

void RegisterState::PrintExceptionReport(const zx_exception_report_t& report) {
  RM_LOG(ERROR) << "type: 0x" << std::hex << report.header.type;
  RM_LOG(ERROR) << "synth_code: 0x" << std::hex << report.context.synth_code;
  RM_LOG(ERROR) << "synth_data: 0x" << std::hex << report.context.synth_data;
  RM_LOG(ERROR) << "ESR: 0x" << std::hex << report.context.arch.u.arm_64.esr;
  RM_LOG(ERROR) << "FAR: 0x" << std::hex << report.context.arch.u.arm_64.far;
}

// This derived class handles the differences between aarch64 and aarch32 in the
// same tests.
class Arch32RegisterState : public RegisterState {
 public:
  void InitializeRegisters() override {
    RegisterState::InitializeRegisters();
    state_.cpsr = 0x10;
  }

  uint64_t instruction_size() const override {
    if (state_.cpsr & 0x20) {  // thumb
      return 2;
    }
    return 4;
  }
  uint64_t register_bytes() const override { return 4; }

  bool ArchSupported() const override {
    uint32_t features;
    zx_status_t status = zx_system_get_features(ZX_FEATURE_KIND_CPU, &features);
    if (status != ZX_OK) {
      RM_LOG(ERROR) << "zx_system_get_features(ZX_FEATURE_KIND_CPU) failed: " << status;
      return false;
    }
    return (features & ZX_ARM64_FEATURE_ISA_ARM32) != 0;
  }

  void InitializeFromThreadState(const zx_thread_state_general_regs_t& regs) override {
    static_assert(sizeof(regs.r) <= sizeof(state_.x));
    ZX_ASSERT((regs.cpsr & 0x10) == 0x10);
    memcpy(state_.x, regs.r, sizeof(regs.r));
    state_.x[14] = regs.lr;
    state_.x[15] = regs.pc;
    state_.pc = regs.pc;
    state_.tpidr_el0 = regs.tpidr;
    state_.sp = regs.sp;
    state_.cpsr = static_cast<uint32_t>(regs.cpsr);
  }

  void set_pc(uintptr_t pc) override {
    state_.pc = pc;
    state_.x[15] = pc;
  }

  void set_sp(uintptr_t sp) override {
    state_.sp = sp;
    state_.x[13] = sp;
  }
  uint64_t syscall_number() const override { return state_.x[7]; }
  void set_syscall_return(uint64_t value) override {
    uint64_t hb = value >> 32;
    if (hb) {
      state_.x[1] = hb;
    }
    state_.x[0] = value;
  }
};

// Map the types to machines.
std::unique_ptr<RegisterState> RegisterStateFactory::Create(const MachineType& machine) {
  if (machine == MachineType::kNative) {
    return std::make_unique<RegisterState>();
  }
  ZX_ASSERT(machine == MachineType::kArm);
  return std::make_unique<Arch32RegisterState>();
}

}  // namespace restricted_machine
