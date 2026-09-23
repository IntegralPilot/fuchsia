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
  registers->tls()->tpidr = 0;
  state->tpidr_el0 = reinterpret_cast<uintptr_t>(&registers->tls()->tpidr);

  // Initialize all standard registers to arbitrary values.
  auto *x = &state->x[0];
  x[0] = 0x0101010101010101;
  x[1] = 0x0202020202020202;
  x[2] = 0x0303030303030303;
  x[3] = 0x0404040404040404;
  x[4] = 0x0505050505050505;
  x[5] = 0x0606060606060606;
  x[6] = 0x0707070707070707;
  x[7] = 0x0808080808080808;
  x[8] = 0x0909090909090909;
  x[9] = 0x0a0a0a0a0a0a0a0a;
  x[10] = 0x0b0b0b0b0b0b0b0b;
  x[11] = 0x0c0c0c0c0c0c0c0c;
  x[12] = 0x0d0d0d0d0d0d0d0d;
  x[13] = 0x0e0e0e0e0e0e0e0e;
  x[14] = 0x0f0f0f0f0f0f0f0f;
  x[15] = 0x0101010101010101;
  x[16] = 0x0202020202020202;
  x[17] = 0x0303030303030303;
  x[18] = 0x0404040404040404;
  x[19] = 0x0505050505050505;
  x[20] = 0x0606060606060606;
  x[21] = 0x0707070707070707;
  x[22] = 0x0808080808080808;
  x[23] = 0x0909090909090909;
  x[24] = 0x0a0a0a0a0a0a0a0a;
  x[25] = 0x0b0b0b0b0b0b0b0b;
  x[26] = 0x0c0c0c0c0c0c0c0c;
  x[27] = 0x0d0d0d0d0d0d0d0d;
  x[28] = 0x0e0e0e0e0e0e0e0e;
  x[29] = 0x0f0f0f0f0f0f0f0f;
  x[30] = 0x0101010101010101;
  // Keep the SP 16-byte aligned, as required by the spec.
  state->sp = 0x0808080808080810;
  state->cpsr = 0;
}

void ArchHelper::VerifyStateMutation(restricted_machine::RegisterState *registers,
                                     RegisterMutation mutation) const {
  auto *state = &registers->restricted_state();
  // Validate the state of the registers is what was written inside restricted mode.
  //
  // NOTE: Each of the registers was incremented by one before exiting restricted mode.
  // x0 was used as temp space by syscall_bounce, so skip that one.
  EXPECT_EQ(0x0202020202020203, state->x[1]);
  EXPECT_EQ(0x0303030303030304, state->x[2]);
  EXPECT_EQ(0x0404040404040405, state->x[3]);
  EXPECT_EQ(0x0505050505050506, state->x[4]);
  EXPECT_EQ(0x0606060606060607, state->x[5]);
  EXPECT_EQ(0x0707070707070708, state->x[6]);
  EXPECT_EQ(0x0808080808080809, state->x[7]);
  EXPECT_EQ(0x090909090909090a, state->x[8]);
  EXPECT_EQ(0x0a0a0a0a0a0a0a0b, state->x[9]);
  EXPECT_EQ(0x0b0b0b0b0b0b0b0c, state->x[10]);
  EXPECT_EQ(0x0c0c0c0c0c0c0c0d, state->x[11]);
  EXPECT_EQ(0x0d0d0d0d0d0d0d0e, state->x[12]);
  EXPECT_EQ(0x0e0e0e0e0e0e0e0f, state->x[13]);
  EXPECT_EQ(0x0f0f0f0f0f0f0f10, state->x[14]);
  EXPECT_EQ(0x0101010101010102, state->x[15]);
  if (mutation == RegisterMutation::kFromSyscall) {
    EXPECT_EQ(0x40, state->x[16]);  // syscall_bounce ran syscall 0x40
  } else {
    EXPECT_EQ(0x0202020202020203, state->x[16]);
  }
  EXPECT_EQ(0x0303030303030304, state->x[17]);
  EXPECT_EQ(0x0404040404040405, state->x[18]);
  EXPECT_EQ(0x0505050505050506, state->x[19]);
  EXPECT_EQ(0x0606060606060607, state->x[20]);
  EXPECT_EQ(0x0707070707070708, state->x[21]);
  EXPECT_EQ(0x0808080808080809, state->x[22]);
  EXPECT_EQ(0x090909090909090a, state->x[23]);
  EXPECT_EQ(0x0a0a0a0a0a0a0a0b, state->x[24]);
  EXPECT_EQ(0x0b0b0b0b0b0b0b0c, state->x[25]);
  EXPECT_EQ(0x0c0c0c0c0c0c0c0d, state->x[26]);
  EXPECT_EQ(0x0d0d0d0d0d0d0d0e, state->x[27]);
  EXPECT_EQ(0x0e0e0e0e0e0e0e0f, state->x[28]);
  EXPECT_EQ(0x0f0f0f0f0f0f0f10, state->x[29]);
  EXPECT_EQ(0x0101010101010102, state->x[30]);
  EXPECT_EQ(0x0808080808080820, state->sp);

  // Check that thread local storage was updated correctly in restricted mode.
  EXPECT_EQ(0x0202020202020203, registers->tls()->tpidr);
}

void ArchHelper::VerifyState(restricted_machine::RegisterState *registers) const {}

class Arch32Helper : public ArchHelper {
 public:
  void SetInitialState(restricted_machine::RegisterState *registers) const override {
    ArchHelper::SetInitialState(registers);
    auto *state = &registers->restricted_state();
    state->cpsr = 0x10;
  }

  void VerifyStateMutation(restricted_machine::RegisterState *registers,
                           RegisterMutation mutation) const override {
    auto *state = &registers->restricted_state();
    // Validate the state of the registers is what was written inside restricted mode.
    //
    // NOTE: Each of the registers was incremented by one before exiting restricted mode.
    // x0 was used as temp space by syscall_bounce, so skip that one.
    EXPECT_EQ(0x0000000002020203, state->x[1]);
    EXPECT_EQ(0x0000000003030304, state->x[2]);
    EXPECT_EQ(0x0000000004040405, state->x[3]);
    EXPECT_EQ(0x0000000005050506, state->x[4]);
    EXPECT_EQ(0x0000000006060607, state->x[5]);
    EXPECT_EQ(0x0000000007070708, state->x[6]);
    EXPECT_EQ(0x000000000909090a, state->x[8]);
    EXPECT_EQ(0x000000000a0a0a0b, state->x[9]);
    EXPECT_EQ(0x000000000b0b0b0c, state->x[10]);
    EXPECT_EQ(0x000000000c0c0c0d, state->x[11]);
    EXPECT_EQ(0x000000000d0d0d0e, state->x[12]);
    if (mutation == RegisterMutation::kFromSyscall) {
      EXPECT_EQ(0x40, state->x[7]);  // syscall_bounce ran syscall 0x40
    } else {
      EXPECT_EQ(0x0000000008080809, state->x[7]);
    }
    EXPECT_EQ(0x000000000e0e0e1e, state->x[13]);  // aarch32 sp
    EXPECT_EQ(0x000000000f0f0f0f, state->x[14]);  // aarch32 lr
    // Registers above 15 will not be saved/updated in restricted state.
    EXPECT_EQ(0x0202020202020202, state->x[16]);  // unchanged

    // Check that thread local storage was updated correctly in restricted mode.
    EXPECT_EQ(0x0000000008080809, registers->tls()->tpidr);
  }
};

// Map the types to machines.
std::unique_ptr<ArchHelper> ArchHelperFactory::Create(
    restricted_machine::MachineType machine_type) {
  if (machine_type == restricted_machine::MachineType::kNative) {
    return std::make_unique<ArchHelper>();
  }
  ZX_ASSERT(machine_type == restricted_machine::MachineType::kArm);
  return std::make_unique<Arch32Helper>();
}
