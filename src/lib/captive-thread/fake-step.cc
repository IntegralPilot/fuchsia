// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fake-step.h"

#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cinttypes>
#include <utility>

#include <hwreg/bitfields.h>

// For RISC-V with no hardware single-step support, fake single-step by
// decoding the instruction enough to set a breakpoint just after it.  For
// branches, set a breakpoint at its target.  For conditional branches, set
// breakpoints in both places.

namespace captive_thread {
namespace {

// Replace the T at vaddr with new_value and return the old value.
// This is used for the breakpoint insertion and removal.
template <typename T>
zx::result<T> ReplaceMemory(uintptr_t vaddr, const T& new_value) {
  T old_value;
  memcpy(&old_value, reinterpret_cast<const T*>(vaddr), sizeof(old_value));
  size_t wrote = 0;
  zx_status_t status =
      zx::process::self()->write_memory(vaddr, &new_value, sizeof(new_value), &wrote);
  if (status != ZX_OK) {
    return zx::error{status};
  }
  if (wrote != sizeof(new_value)) {
    return zx::error{ZX_ERR_IO};
  }
  return zx::ok(old_value);
}

// Fetch the T at vaddr, for the instruction decoder.
template <typename T>
T Fetch(uintptr_t pc) {
  T insn;
  // The item is not necessarily naturally-aligned, so don't use T* in the
  // source type.  In RVC, the 32-bit (non-C) instruction encoding can appear
  // aligned only to 16 bits.
  memcpy(&insn, reinterpret_cast<const std::byte*>(pc), sizeof(insn));
  return insn;
}

// Helper function for the decoder.
constexpr uint64_t Reg(const zx_riscv64_thread_state_general_regs_t& regs, uint32_t i) {
  std::span<const uint64_t, 32> x{&regs.pc, 32};
  return i == 0 ? 0 : x[i];
}

template <uint8_t Bits>
  requires(Bits > 0 && Bits < 32)
constexpr int32_t SignExtend(uint32_t x) {
  constexpr int kShift = 32 - Bits;
  return static_cast<int32_t>(x << kShift) >> kShift;
}

// This describes a 32-bit RISC-V instruction.  It only has the formats that
// are used for jump instructions.
union RiscvInsn {
  uint32_t insn;
  DEF_SUBFIELD(insn, 6, 0, opcode);

  // jalr (jr) uses I format.
  struct {
    constexpr uintptr_t jump_target(const zx_riscv64_thread_state_general_regs_t& regs) const {
      return (Reg(regs, rs1()) + SignExtend<12>(imm())) & ~uintptr_t{1};
    }

    uint32_t insn;
    DEF_SUBFIELD(insn, 31, 20, imm);
    DEF_SUBFIELD(insn, 19, 15, rs1);
    DEF_SUBFIELD(insn, 14, 12, funct3);
    DEF_SUBFIELD(insn, 11, 7, rd);
    DEF_SUBFIELD(insn, 6, 0, opcode);
  } i;

  // The conditional branches use B format.
  struct {
    constexpr uintptr_t jump_target(uintptr_t pc) const { return pc + SignExtend<13>(imm()); }

    constexpr uint32_t imm() const {
      return (imm12() << 12) | (imm11() << 11) | (imm10_5() << 5) | (imm4_1() << 1);
    }

    uint32_t insn;
    DEF_SUBFIELD(insn, 31, 31, imm12);
    DEF_SUBFIELD(insn, 30, 25, imm10_5);
    DEF_SUBFIELD(insn, 24, 20, rs2);
    DEF_SUBFIELD(insn, 19, 15, rs1);
    DEF_SUBFIELD(insn, 14, 12, funct3);
    DEF_SUBFIELD(insn, 11, 8, imm4_1);
    DEF_SUBFIELD(insn, 7, 7, imm11);
    DEF_SUBFIELD(insn, 6, 0, opcode);
  } b;

  // jal (j) uses J format.
  struct {
    constexpr uintptr_t jump_target(uintptr_t pc) const { return pc + SignExtend<21>(imm()); }

    constexpr uint32_t imm() const {
      return (imm20() << 20) | (imm19_12() << 12) | (imm11() << 11) | (imm10_1() << 1);
    }

    uint32_t insn;
    DEF_SUBFIELD(insn, 31, 31, imm20);
    DEF_SUBFIELD(insn, 30, 21, imm10_1);
    DEF_SUBFIELD(insn, 20, 20, imm11);
    DEF_SUBFIELD(insn, 19, 12, imm19_12);
    DEF_SUBFIELD(insn, 11, 7, rd);
    DEF_SUBFIELD(insn, 6, 0, opcode);
  } j;
};

// This describes a 16-bit RISC-V extension C instruction.  It only has the
// formats that are used for jump instructions.
union RiscvCInsn {
  uint16_t insn;
  DEF_SUBFIELD(insn, 1, 0, op);

  // c.jalr (c.jr) uses CR format.
  struct {
    constexpr uintptr_t jump_target(const zx_riscv64_thread_state_general_regs_t& regs) const {
      return Reg(regs, rs1());
    }

    uint16_t insn;
    DEF_SUBFIELD(insn, 15, 12, funct4);
    DEF_SUBFIELD(insn, 11, 7, rs1);
    DEF_SUBFIELD(insn, 6, 2, rs2);
    DEF_SUBFIELD(insn, 1, 0, op);
  } cr;

  // c.beqz / c.bnez use CB format.
  struct {
    constexpr uintptr_t jump_target(uintptr_t pc) const { return pc + SignExtend<9>(imm()); }

    constexpr uint32_t imm() const {
      return (imm8() << 8) | (imm7_6() << 6) | (imm5() << 5) | (imm4_3() << 3) | (imm2_1() << 1);
    }

    uint16_t insn;
    DEF_SUBFIELD(insn, 15, 13, funct3);
    DEF_SUBFIELD(insn, 12, 12, imm8);
    DEF_SUBFIELD(insn, 11, 10, imm4_3);
    DEF_SUBFIELD(insn, 9, 7, rd);
    DEF_SUBFIELD(insn, 6, 5, imm7_6);
    DEF_SUBFIELD(insn, 4, 3, imm2_1);
    DEF_SUBFIELD(insn, 2, 2, imm5);
    DEF_SUBFIELD(insn, 1, 0, op);
  } cb;

  // c.j uses CJ format.
  struct {
    constexpr uintptr_t jump_target(uintptr_t pc) const { return pc + SignExtend<12>(imm()); }

    constexpr uint32_t imm() const {
      return (imm11() << 11) | (imm10() << 10) | (imm9_8() << 8) | (imm7() << 7) | (imm6() << 6) |
             (imm5() << 5) | (imm4() << 4) | (imm3_1() << 1);
    }

    uint16_t insn;
    DEF_SUBFIELD(insn, 15, 13, funct3);
    DEF_SUBFIELD(insn, 12, 12, imm11);
    DEF_SUBFIELD(insn, 11, 11, imm4);
    DEF_SUBFIELD(insn, 10, 9, imm9_8);
    DEF_SUBFIELD(insn, 8, 8, imm10);
    DEF_SUBFIELD(insn, 7, 7, imm6);
    DEF_SUBFIELD(insn, 6, 6, imm7);
    DEF_SUBFIELD(insn, 5, 3, imm3_1);
    DEF_SUBFIELD(insn, 2, 2, imm5);
    DEF_SUBFIELD(insn, 1, 0, op);
  } cj;
};

// Any one instruction has at most two possible successor PCs.
using Locations = cpp26::inplace_vector<uintptr_t, 2>;

// A conditional branch has two successors unless they're identical.
constexpr Locations Conditional(uintptr_t first, uintptr_t second) {
  return first == second ? Locations{first} : Locations{first, second};
}

// Return one or both successor PCs for regs.pc by decoding the instruction.
Locations Successors(const zx_riscv64_thread_state_general_regs_t& regs) {
  ZX_DEBUG_ASSERT_MSG(regs.pc % 2 == 0, ": PC=%#" PRIx64, regs.pc);
  const auto c_insn = Fetch<RiscvCInsn>(regs.pc);
  switch (c_insn.op()) {
    case 0b11: {  // 32-bit-wide instruction
      const uintptr_t next = regs.pc + 4;
      const auto insn = Fetch<RiscvInsn>(regs.pc);
      switch (insn.opcode() >> 2) {
        case 0b11000:  // B
          return Conditional(next, insn.b.jump_target(regs.pc));
        case 0b11011:  // jal (J)
          return {insn.j.jump_target(regs.pc)};
        case 0b11001:  // jalr (I)
          return {insn.i.jump_target(regs)};
        default:
          return {next};
      }
    }

    case 0b00:  // C instructions, no branches here.
      break;

    case 0b01:  // CJ / CB
      switch (c_insn.cj.funct3()) {
        case 0b101:  // c.j
          return {c_insn.cj.jump_target(regs.pc)};
        case 0b110:  // c.beqz
        case 0b111:  // c.bnez
          return Conditional(regs.pc + 2, c_insn.cb.jump_target(regs.pc));
        default:
          break;
      }
      break;

    case 0b10:  // CR
      if (c_insn.cr.funct4() == 0b1000 && c_insn.cr.rs1() != 0 && c_insn.cr.rs2() == 0) {
        // c.jr / c.jalr
        return {c_insn.cr.jump_target(regs)};
      }
      break;

    default:
      std::unreachable();
  }
  return {regs.pc + 2};
}

}  // namespace

zx::result<> CaptiveThread::FakeStep::SetBreakpoints(
    const zx_riscv64_thread_state_general_regs_t& regs) {
  ZX_DEBUG_ASSERT(breakpoints_.empty());
  for (auto location : Successors(regs)) {
    zx::result insn = ReplaceMemory(location, kBreakpointInsn);
    if (insn.is_error()) {
      return insn.take_error();
    }
    breakpoints_.push_back({.location = location, .saved = *insn});
  }
  return zx::ok();
}

zx::result<> CaptiveThread::FakeStep::ClearBreakpoints() {
  zx::result<> result = zx::ok();
  for (auto [location, saved] : std::exchange(breakpoints_, {})) {
    zx::result clear = ReplaceMemory(location, saved);
    if (clear.is_ok()) {
      ZX_DEBUG_ASSERT(*clear == kBreakpointInsn);
    } else if (result.is_ok()) {
      result = clear.take_error();
    }
  }
  return result;
}

void CaptiveThread::FakeStep::FixupException(  //
    CaptiveThread& thread, zx_exception_report_t& report) const {
  if (report.header.type != ZX_EXCP_SW_BREAKPOINT) {
    return;
  }
  zx::result regs = thread.Registers();
  if (regs.is_error()) {
    return;
  }
  for (const auto& bkpt : breakpoints_) {
    if (regs->pc == bkpt.location) {
      report.header.type = kSingleStepException;
      return;
    }
  }
}

}  // namespace captive_thread
