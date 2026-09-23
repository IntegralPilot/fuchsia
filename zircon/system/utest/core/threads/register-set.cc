// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "register-set.h"

#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>
#include <zircon/hw/debug/arm64.h>

#include <cstdint>

#include <zxtest/zxtest.h>

#ifdef __riscv

constexpr size_t kRiscvVlenb = 16;

#endif

namespace {

// Write a NaN double value to the given uint64_t (which is how most of the
// registers are stored in the structs).
void WriteNaNDouble(uint64_t* output) {
  double nan_value = nan("");
  memcpy(output, &nan_value, sizeof(double));
}

}  // namespace

// Fill Test Values -------------------------------------------------------------------------------

void general_regs_fill_test_values(zx_thread_state_general_regs_t* regs) {
  for (uint32_t index = 0; index < sizeof(*regs); ++index) {
    ((uint8_t*)regs)[index] = static_cast<uint8_t>(index + 1);
  }

  // Set various flags bits that will read back the same.
#if defined(__x86_64__)

  // Here we set all flag bits that are modifiable from user space or
  // that are not modifiable but are expected to read back as 1, with the
  // exception of the trap flag (bit 8, which would interfere with
  // execution if we set it).
  //
  // Note that setting the direction flag (bit 10) helps test whether the
  // kernel correctly handles taking an interrupt when that flag is set
  // (see https://fxbug.dev/42105863).
  regs->rflags = (1 << 0) |   // CF: carry flag
                 (1 << 1) |   // Reserved, always 1
                 (1 << 2) |   // PF: parity flag
                 (1 << 4) |   // AF: adjust flag
                 (1 << 6) |   // ZF: zero flag
                 (1 << 7) |   // SF: sign flag
                 (1 << 9) |   // IF: interrupt enable flag (set by kernel)
                 (1 << 10) |  // DF: direction flag
                 (1 << 11) |  // OF: overflow flag
                 (1 << 14) |  // NT: nested task flag
                 (1 << 18) |  // AC: alignment check flag
                 (1 << 21);   // ID: used for testing for CPUID support

  // Set these to canonical addresses to avoid an error.
  regs->fs_base = 0x0;
  regs->gs_base = 0x0;
  regs->rip = 0x0;

#elif defined(__aarch64__)

  // Only set the 4 flag bits that are readable and writable by the
  // instructions "msr nzcv, REG" and "mrs REG, nzcv".
  regs->cpsr = 0xf0000000;
  regs->tpidr = 0;

#elif defined(__riscv)

  // spin_with_general_regs needs t0 for its final jump, so it doesn't have the
  // arbitrary value set generically above.
  regs->t0 = reinterpret_cast<uintptr_t>(spin_address);

#endif
}

void fp_regs_fill_test_values(zx_thread_state_fp_regs_t* regs) {
  memset(regs, 0, sizeof(zx_thread_state_fp_regs_t));

#if defined(__x86_64__)

  for (size_t i = 0; i < 7; i++) {
    regs->st[i].low = i;
  }

  // Write NaN to the last value.
  WriteNaNDouble(&regs->st[7].low);

#elif defined(__aarch64__)

  // No FP struct on ARM (vector only).

#elif defined(__riscv)

  for (size_t i = 0; i < 32; ++i) {
    regs->q[i].low = i;

    // The high half of Q registers is not really supported yet.
    // That's indicated with "NaN-boxing", i.e. all ones in the high half.
    regs->q[i].high = ~uint64_t{};
  }

  // Write NaN to the last value.
  WriteNaNDouble(&regs->q[31].low);

#else

#error Unsupported architecture

#endif
}

void vector_regs_fill_test_values(zx_thread_state_vector_regs_t* regs) {
  memset(regs, 0, sizeof(zx_thread_state_vector_regs_t));

#if defined(__x86_64__)

  for (uint64_t i = 0; i < 16; i++) {
    // Only sets the XMM registers (first two) since that's all that's guaranteed.
    regs->zmm[i].v[0] = i;
    regs->zmm[i].v[1] = i << 8;
    regs->zmm[i].v[2] = 0;
    regs->zmm[i].v[3] = 0;
  }

  // Write NaN to the last value.
  WriteNaNDouble(&regs->zmm[15].v[0]);

#elif defined(__aarch64__)

  for (uint64_t i = 0; i < 32; i++) {
    regs->v[i].low = i;
    regs->v[i].high = i << 8;
  }

  // Write NaN to the last value.
  WriteNaNDouble(&regs->v[31].low);

#elif defined(__riscv)

  // As a mildly interesting test case, we set an element width of 64 bytes and
  // a grouping of 4 vector registers per operation, operating on up to 4
  // elements at a time.
  constexpr uint64_t kSew64 = 0b011 << 3;
  constexpr uint64_t kLmul4 = 0b010;
  const uint64_t vlmax = (kRiscvVlenb * 8) * 4 / 64;
  regs->vcsr = 0;
  regs->vstart = 0;
  regs->vtype = kSew64 | kLmul4;
  regs->vl = vlmax / 2;
  for (uint8_t i = 0; i < 32; i++) {
    static_assert(sizeof(regs->v[i]) == 16);
    for (uint8_t j = 0; j < kRiscvVlenb; j++) {
      regs->v[i].contents[j] = j == 0 ? i : j;
    }
  }

#else

#error Unsupported architecture

#endif
}

void debug_regs_fill_test_values(zx_thread_state_debug_regs_t* to_write,
                                 zx_thread_state_debug_regs_t* expected) {
  [[maybe_unused]] const uint64_t base = reinterpret_cast<uintptr_t>(debug_regs_fill_test_values);

#if defined(__x86_64__)

  // The kernel will validate that the addresses set into the debug registers are valid userspace
  // one. We use values relative to this function, as it is guaranteed to be in the userspace
  // range.
  to_write->dr[0] = base;
  to_write->dr[1] = base + 0x4000;
  to_write->dr[2] = base + 0x8000;
  to_write->dr[3] = 0x0;  // Zero is also valid.
  to_write->dr6 = 0;
  to_write->dr7 = 0x33;  // Activate all breakpoints.

  expected->dr[0] = base;
  expected->dr[1] = base + 0x4000;
  expected->dr[2] = base + 0x8000;
  expected->dr[3] = 0x0;
  expected->dr6 = 0xffff0ff0;  // No breakpoint event detected.
  expected->dr7 = 0x733;       // Activate all breakpoints.

#elif defined(__aarch64__)

  *to_write = {};

  // We only set two because we know that arm64 ensures that.
  ARM64_DBGBCR_E_SET(&to_write->hw_bps[0].dbgbcr, 1);
  ARM64_DBGBCR_E_SET(&to_write->hw_bps[1].dbgbcr, 1);
  to_write->hw_bps[0].dbgbvr = base;
  to_write->hw_bps[1].dbgbvr = base + 0x4000;

  ARM64_DBGWCR_E_SET(&to_write->hw_wps[0].dbgwcr, 1);
  ARM64_DBGWCR_BAS_SET(&to_write->hw_wps[0].dbgwcr, 0xf);
  ARM64_DBGWCR_LSC_SET(&to_write->hw_wps[0].dbgwcr, 0b11);
  ARM64_DBGWCR_E_SET(&to_write->hw_wps[1].dbgwcr, 1);
  ARM64_DBGWCR_BAS_SET(&to_write->hw_wps[1].dbgwcr, 0xf0);
  to_write->hw_wps[0].dbgwvr = base;
  to_write->hw_wps[1].dbgwvr = base + 0x4000;

  *expected = *to_write;
  ARM64_DBGBCR_PMC_SET(&expected->hw_bps[0].dbgbcr, 0b10);
  ARM64_DBGBCR_BAS_SET(&expected->hw_bps[0].dbgbcr, 0xf);
  ARM64_DBGBCR_PMC_SET(&expected->hw_bps[1].dbgbcr, 0b10);
  ARM64_DBGBCR_BAS_SET(&expected->hw_bps[1].dbgbcr, 0xf);

  ARM64_DBGWCR_PAC_SET(&expected->hw_wps[0].dbgwcr, 0b10);
  ARM64_DBGWCR_LSC_SET(&expected->hw_wps[0].dbgwcr, 0b11);
  ARM64_DBGWCR_SSC_SET(&expected->hw_wps[0].dbgwcr, 1);
  ARM64_DBGWCR_PAC_SET(&expected->hw_wps[1].dbgwcr, 0b10);
  ARM64_DBGWCR_LSC_SET(&expected->hw_wps[1].dbgwcr, 0);
  ARM64_DBGWCR_SSC_SET(&expected->hw_wps[1].dbgwcr, 1);

#elif defined(__riscv)

  // No hardware watchpoint support on RISC-V yet.

#else

#error Unsupported architecture

#endif
}

// Expect Eq Functions ----------------------------------------------------------------------------

void general_regs_expect_eq(const zx_thread_state_general_regs_t& regs1,
                            const zx_thread_state_general_regs_t& regs2) {
#define CHECK_REG(FIELD) EXPECT_EQ(regs1.FIELD, regs2.FIELD, "Reg " #FIELD)

#if defined(__x86_64__)

  CHECK_REG(rax);
  CHECK_REG(rbx);
  CHECK_REG(rcx);
  CHECK_REG(rdx);
  CHECK_REG(rsi);
  CHECK_REG(rdi);
  CHECK_REG(rbp);
  CHECK_REG(rsp);
  CHECK_REG(r8);
  CHECK_REG(r9);
  CHECK_REG(r10);
  CHECK_REG(r11);
  CHECK_REG(r12);
  CHECK_REG(r13);
  CHECK_REG(r14);
  CHECK_REG(r15);
  CHECK_REG(rip);
  CHECK_REG(rflags);

#elif defined(__aarch64__)

  for (int regnum = 0; regnum < 30; ++regnum) {
    EXPECT_EQ(regs1.r[regnum], regs2.r[regnum], "Reg r[%d]", regnum);
  }
  CHECK_REG(lr);
  CHECK_REG(sp);
  CHECK_REG(pc);
  CHECK_REG(cpsr);

#elif defined(__riscv)

  CHECK_REG(pc);
  CHECK_REG(ra);   // x1
  CHECK_REG(sp);   // x2
  CHECK_REG(gp);   // x3
  CHECK_REG(tp);   // x4
  CHECK_REG(t0);   // x5
  CHECK_REG(t1);   // x6
  CHECK_REG(t2);   // x7
  CHECK_REG(s0);   // x8
  CHECK_REG(s1);   // x9
  CHECK_REG(a0);   // x10
  CHECK_REG(a1);   // x11
  CHECK_REG(a2);   // x12
  CHECK_REG(a3);   // x13
  CHECK_REG(a4);   // x14
  CHECK_REG(a5);   // x15
  CHECK_REG(a6);   // x16
  CHECK_REG(a7);   // x17
  CHECK_REG(s2);   // x18
  CHECK_REG(s3);   // x19
  CHECK_REG(s4);   // x20
  CHECK_REG(s5);   // x21
  CHECK_REG(s6);   // x22
  CHECK_REG(s7);   // x23
  CHECK_REG(s8);   // x24
  CHECK_REG(s9);   // x25
  CHECK_REG(s10);  // x26
  CHECK_REG(s11);  // x27
  CHECK_REG(t3);   // x28
  CHECK_REG(t4);   // x29
  CHECK_REG(t5);   // x30
  CHECK_REG(t6);   // x31

#else

#error Unsupported architecture

#endif

#undef CHECK_REG
}

void fp_regs_expect_eq(const zx_thread_state_fp_regs_t& regs1,
                       const zx_thread_state_fp_regs_t& regs2) {
#if defined(__x86_64__)

  // This just tests the MMX registers.
  EXPECT_EQ(regs1.st[0].low, regs2.st[0].low, "Reg st[0].low");
  EXPECT_EQ(regs1.st[1].low, regs2.st[1].low, "Reg st[1].low");
  EXPECT_EQ(regs1.st[2].low, regs2.st[2].low, "Reg st[2].low");
  EXPECT_EQ(regs1.st[3].low, regs2.st[3].low, "Reg st[3].low");
  EXPECT_EQ(regs1.st[4].low, regs2.st[4].low, "Reg st[4].low");
  EXPECT_EQ(regs1.st[5].low, regs2.st[5].low, "Reg st[5].low");
  EXPECT_EQ(regs1.st[6].low, regs2.st[6].low, "Reg st[6].low");
  EXPECT_EQ(regs1.st[7].low, regs2.st[7].low, "Reg st[7].low");

#elif defined(__aarch64__)

  // No FP regs on ARM (uses vector regs for FP).
  (void)regs1;
  (void)regs2;

#elif defined(__riscv)

  for (size_t i = 0; i < 32; ++i) {
    EXPECT_EQ(regs1.q[i].low, regs2.q[i].low);
    EXPECT_EQ(regs1.q[i].high, regs2.q[i].high);
  }

  EXPECT_EQ(regs1.fcsr, regs2.fcsr);

#else

#error Unsupported architecture

#endif
}

void vector_regs_expect_unsupported_are_zero(const zx_thread_state_vector_regs_t& regs) {
#if defined(__x86_64__)

  // For the first 16 ZMM registers, we currently support only the lowest 256-bits.  All others
  // should be 0.
  for (int reg = 0; reg < 16; reg++) {
    for (int i = 4; i < 8; i++) {
      EXPECT_EQ(regs.zmm[reg].v[i], 0);
    }
  }
  // The next 16 ZMM registers are unsupported.
  for (int reg = 16; reg < 32; reg++) {
    for (int i = 0; i < 8; i++) {
      EXPECT_EQ(regs.zmm[reg].v[i], 0);
    }
  }

#elif defined(__aarch64__)

  // All features/fields are supported on arm64.

#elif defined(__riscv)

  // All RISC-V vector registers are supported.

#else

#error Unsupported architecture

#endif
}

void vector_regs_expect_eq(const zx_thread_state_vector_regs_t& regs1,
                           const zx_thread_state_vector_regs_t& regs2) {
#if defined(__x86_64__)

  // Only check the first 16 registers (guaranteed to work).
  for (int reg = 0; reg < 16; reg++) {
    // Only check the low 128 bits (guaranteed to work).
    EXPECT_EQ(regs1.zmm[reg].v[0], regs2.zmm[reg].v[0]);
    EXPECT_EQ(regs1.zmm[reg].v[1], regs2.zmm[reg].v[1]);
  }

#elif defined(__aarch64__)

  for (int i = 0; i < 32; i++) {
    EXPECT_EQ(regs1.v[i].high, regs2.v[i].high);
    EXPECT_EQ(regs1.v[i].low, regs2.v[i].low);
  }

#elif defined(__riscv)
  for (size_t i = 0; i < 32; i++) {
    EXPECT_BYTES_EQ(regs2.v[i].contents, regs1.v[i].contents, kRiscvVlenb, "(i = %zu)", i);
  }
  EXPECT_EQ(regs2.vcsr, regs1.vcsr);
  EXPECT_EQ(regs2.vl, regs1.vl);
  EXPECT_EQ(regs2.vstart, regs1.vstart);
  EXPECT_EQ(regs2.vtype, regs1.vtype);

#else

#error Unsupported architecture

#endif
}

void debug_regs_expect_eq(const char* file, int line, const zx_thread_state_debug_regs_t& regs1,
                          const zx_thread_state_debug_regs_t& regs2) {
#if defined(__x86_64__)
  EXPECT_EQ(regs1.dr[0], regs2.dr[0], "%s:%d: %s", file, line, "Reg DR0");
  EXPECT_EQ(regs1.dr[1], regs2.dr[1], "%s:%d: %s", file, line, "Reg DR1");
  EXPECT_EQ(regs1.dr[2], regs2.dr[2], "%s:%d: %s", file, line, "Reg DR2");
  EXPECT_EQ(regs1.dr[3], regs2.dr[3], "%s:%d: %s", file, line, "Reg DR3");
  EXPECT_EQ(regs1.dr6, regs2.dr6, "%s:%d: %s", file, line, "Reg DR6");
  EXPECT_EQ(regs1.dr7, regs2.dr7, "%s:%d: %s", file, line, "Reg DR7");

#elif defined(__aarch64__)

  for (uint32_t i = 0; i < 16; i++) {
    EXPECT_EQ(regs1.hw_bps[i].dbgbcr, regs2.hw_bps[i].dbgbcr);
    EXPECT_EQ(regs1.hw_bps[i].dbgbvr, regs2.hw_bps[i].dbgbvr);
  }

  for (uint32_t i = 0; i < 16; i++) {
    EXPECT_EQ(regs1.hw_wps[i].dbgwcr, regs2.hw_wps[i].dbgwcr);
    EXPECT_EQ(regs1.hw_wps[i].dbgwvr, regs2.hw_wps[i].dbgwvr);
  }

  EXPECT_EQ(regs1.esr, regs2.esr);
  EXPECT_EQ(regs1.far, regs2.far);

#elif defined(__riscv)

  // No hardware watchpoint support on RISC-V yet.

#else

#error Unsupported architecture

#endif
}
