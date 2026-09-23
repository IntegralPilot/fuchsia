// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "arch-helper.h"

#include <bringup/lib/restricted-machine/machine-type.h>
#include <bringup/lib/restricted-machine/register-state.h>
#include <bringup/lib/restricted-machine/tls-storage.h>
#include <zxtest/zxtest.h>

void ArchHelper::SetInitialState(restricted_machine::RegisterState *registers) const {
  auto *state = &registers->restricted_state();
  registers->tls()->fs_val = 0;
  registers->tls()->gs_val = 0;
  state->fs_base = reinterpret_cast<uintptr_t>(&registers->tls()->fs_val);
  state->gs_base = reinterpret_cast<uintptr_t>(&registers->tls()->gs_val);

  // Initialize all standard registers to arbitrary values.
  state->rdi = 0x0606060606060606;
  state->rsi = 0x0505050505050505;
  state->rbp = 0x0707070707070707;
  state->rbx = 0x0202020202020202;
  state->rdx = 0x0404040404040404;
  state->rcx = 0x0303030303030303;
  state->rax = 0x0101010101010101;
  state->rsp = 0x0808080808080808;
  state->r8 = 0x0909090909090909;
  state->r9 = 0x0a0a0a0a0a0a0a0a;
  state->r10 = 0x0b0b0b0b0b0b0b0b;
  state->r11 = 0x0c0c0c0c0c0c0c0c;
  state->r12 = 0x0d0d0d0d0d0d0d0d;
  state->r13 = 0x0e0e0e0e0e0e0e0e;
  state->r14 = 0x0f0f0f0f0f0f0f0f;
  state->r15 = 0x1010101010101010;
  state->flags = 0;
}

void ArchHelper::VerifyStateMutation(restricted_machine::RegisterState *registers,
                                     RegisterMutation mutation) const {
  auto *state = &registers->restricted_state();
  // Validate the state of the registers is what was written inside restricted mode.
  //
  // NOTE: Each of the registers was incremented by one before exiting restricted mode.
  EXPECT_EQ(0x0101010101010102, state->rax);
  EXPECT_EQ(0x0202020202020203, state->rbx);
  if (mutation == RegisterMutation::kFromSyscall) {
    EXPECT_EQ(0, state->rcx);  // RCX is trashed by the syscall and set to zero
  } else {
    EXPECT_EQ(0x0303030303030304, state->rcx);
  }
  EXPECT_EQ(0x0404040404040405, state->rdx);
  EXPECT_EQ(0x0505050505050506, state->rsi);
  EXPECT_EQ(0x0606060606060607, state->rdi);
  EXPECT_EQ(0x0707070707070708, state->rbp);
  EXPECT_EQ(0x0808080808080809, state->rsp);
  EXPECT_EQ(0x090909090909090a, state->r8);
  EXPECT_EQ(0x0a0a0a0a0a0a0a0b, state->r9);
  EXPECT_EQ(0x0b0b0b0b0b0b0b0c, state->r10);
  if (mutation == RegisterMutation::kFromSyscall) {
    EXPECT_EQ(0, state->r11);  // r11 is trashed by the syscall and set to zero
  } else {
    EXPECT_EQ(0x0c0c0c0c0c0c0c0d, state->r11);
  }
  EXPECT_EQ(0x0d0d0d0d0d0d0d0e, state->r12);
  EXPECT_EQ(0x0e0e0e0e0e0e0e0f, state->r13);
  EXPECT_EQ(0x0f0f0f0f0f0f0f10, state->r14);
  EXPECT_EQ(0x1010101010101011, state->r15);

  // Validate that it was able to write to fs:0 and gs:0 while inside restricted mode the post
  // incremented values of rcx and r11 were written here.
  EXPECT_EQ(0x0303030303030304, registers->tls()->fs_val);
  EXPECT_EQ(0x0c0c0c0c0c0c0c0d, registers->tls()->gs_val);
}

void ArchHelper::VerifyState(restricted_machine::RegisterState *registers) const {
  // Verify that the flags field does not contain reserved bits. These are rejected by
  // zx_restricted_enter.
  // [intel/vol1]: 3.4.3 EFLAGS Register: Bits 1, 3, 5, 15, and 22 through 31 of this register are
  // reserved. Software should not use or depend on the states of any of these bits.
  constexpr uint64_t kX86ReservedFlagBitss =
      0b11111111'11000000'10000000'00101010 | (0xffffffffull << 32);
  EXPECT_EQ(registers->restricted_state().flags & kX86ReservedFlagBitss, 0);
}

// Map the types to machines.
std::unique_ptr<ArchHelper> ArchHelperFactory::Create(
    restricted_machine::MachineType machine_type) {
  assert(machine_type == restricted_machine::MachineType::kNative);
  return std::make_unique<ArchHelper>();
}
