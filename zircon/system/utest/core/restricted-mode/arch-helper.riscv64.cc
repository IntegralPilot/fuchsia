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
  // Configure TLS storage.
  registers->tls()->tp = 0;
  state->tp = reinterpret_cast<uintptr_t>(&registers->tls()->tp);

  // Initialize all standard registers to arbitrary values.
  state->ra = 0x0101010101010101;
  state->sp = 0x0202020202020202;
  state->gp = 0x0303030303030303;
  state->t0 = 0x0505050505050505;
  state->t1 = 0x0606060606060606;
  state->t2 = 0x0707070707070707;
  state->s0 = 0x0808080808080808;
  state->s1 = 0x0909090909090909;
  state->a0 = 0x0a0a0a0a0a0a0a0a;
  state->a1 = 0x0b0b0b0b0b0b0b0b;
  state->a2 = 0x0c0c0c0c0c0c0c0c;
  state->a3 = 0x0d0d0d0d0d0d0d0d;
  state->a4 = 0x0e0e0e0e0e0e0e0e;
  state->a5 = 0x0f0f0f0f0f0f0f0f;
  state->a6 = 0x0101010101010101;
  state->a7 = 0x0202020202020202;
  state->s2 = 0x0303030303030303;
  state->s3 = 0x0404040404040404;
  state->s4 = 0x0505050505050505;
  state->s5 = 0x0606060606060606;
  state->s6 = 0x0707070707070707;
  state->s7 = 0x0808080808080808;
  state->s8 = 0x0909090909090909;
  state->s9 = 0x0a0a0a0a0a0a0a0a;
  state->s10 = 0x0b0b0b0b0b0b0b0b;
  state->s11 = 0x0c0c0c0c0c0c0c0c;
  state->t3 = 0x0d0d0d0d0d0d0d0d;
  state->t4 = 0x0e0e0e0e0e0e0e0e;
  state->t5 = 0x0f0f0f0f0f0f0f0f;
  state->t6 = 0x0101010101010101;
}

void ArchHelper::VerifyStateMutation(restricted_machine::RegisterState *registers,
                                     RegisterMutation mutation) const {
  auto *state = &registers->restricted_state();
  // Validate the state of the registers is what was written inside restricted mode.
  //
  // NOTE: Each of the registers was incremented by one before exiting restricted mode.
  EXPECT_EQ(0x0101010101010102, state->ra);
  EXPECT_EQ(0x0202020202020203, state->sp);
  EXPECT_EQ(0x0303030303030304, state->gp);
  EXPECT_EQ(reinterpret_cast<uintptr_t>(&registers->tls()->tp), state->tp);
  if (mutation == RegisterMutation::kFromSyscall) {
    EXPECT_EQ(0x40, state->t0);
  } else {
    EXPECT_EQ(0x0505050505050506, state->t0);
  }
  EXPECT_EQ(0x0606060606060607, state->t1);
  EXPECT_EQ(0x0707070707070708, state->t2);
  EXPECT_EQ(0x0808080808080809, state->s0);
  EXPECT_EQ(0x090909090909090a, state->s1);
  EXPECT_EQ(0x0a0a0a0a0a0a0a0b, state->a0);
  EXPECT_EQ(0x0b0b0b0b0b0b0b0c, state->a1);
  EXPECT_EQ(0x0c0c0c0c0c0c0c0d, state->a2);
  EXPECT_EQ(0x0d0d0d0d0d0d0d0e, state->a3);
  EXPECT_EQ(0x0e0e0e0e0e0e0e0f, state->a4);
  EXPECT_EQ(0x0f0f0f0f0f0f0f10, state->a5);
  EXPECT_EQ(0x0101010101010102, state->a6);
  EXPECT_EQ(0x0202020202020203, state->a7);
  EXPECT_EQ(0x0303030303030304, state->s2);
  EXPECT_EQ(0x0404040404040405, state->s3);
  EXPECT_EQ(0x0505050505050506, state->s4);
  EXPECT_EQ(0x0606060606060607, state->s5);
  EXPECT_EQ(0x0707070707070708, state->s6);
  EXPECT_EQ(0x0808080808080809, state->s7);
  EXPECT_EQ(0x090909090909090a, state->s8);
  EXPECT_EQ(0x0a0a0a0a0a0a0a0b, state->s9);
  EXPECT_EQ(0x0b0b0b0b0b0b0b0c, state->s10);
  EXPECT_EQ(0x0c0c0c0c0c0c0c0d, state->s11);
  EXPECT_EQ(0x0d0d0d0d0d0d0d0e, state->t3);
  EXPECT_EQ(0x0e0e0e0e0e0e0e0f, state->t4);
  EXPECT_EQ(0x0f0f0f0f0f0f0f10, state->t5);
  EXPECT_EQ(0x0101010101010102, state->t6);

  // Check that thread local storage was updated correctly in restricted mode.
  EXPECT_EQ(0x0505050505050506, registers->tls()->tp);
}

void ArchHelper::VerifyState(restricted_machine::RegisterState *registers) const {}

// Map the types to machines.
std::unique_ptr<ArchHelper> ArchHelperFactory::Create(
    restricted_machine::MachineType machine_type) {
  assert(machine_type == restricted_machine::MachineType::kNative);
  return std::make_unique<ArchHelper>();
}
