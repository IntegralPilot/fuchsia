// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/riscv64/system.h>

#include <phys/exception.h>

#include "riscv64.h"

// The ArchPhysExceptionEntry assembly code stored all the registers.
// Fetch the CSRs and classify the exception.
PHYS_SINGLETHREAD uint64_t ArchPhysException(PhysExceptionState& state) {
  auto cause = arch::RiscvScause::Read();
  auto tval = arch::RiscvStval::Read();
  state.exc = {};
  state.exc.arch.u.riscv_64.cause = cause.reg_value();
  state.exc.arch.u.riscv_64.tval = tval.reg_value();
  return PhysException(cause.reg_value(), cause.description(), state);
}

uint64_t PhysExceptionResume(PhysExceptionState& regs, uint64_t pc, uint64_t sp, uint64_t psr) {
  regs.regs.pc = pc;
  regs.regs.sp = sp;
  return PHYS_EXCEPTION_RESUME;
}
