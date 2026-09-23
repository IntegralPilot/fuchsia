// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_VECTOR_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_VECTOR_H_

// Offsets of riscv64_vector_state fields for use in assembly. These are
// statically asserted as correct below.
#define RISCV64_VECTOR_STATE_V 0x0
#define RISCV64_VECTOR_STATE_VCSR 0x200
#define RISCV64_VECTOR_STATE_VSTART 0x208
#define RISCV64_VECTOR_STATE_VL 0x210
#define RISCV64_VECTOR_STATE_VTYPE 0x218

#ifndef __ASSEMBLER__

#include <stdint.h>
#include <zircon/syscalls/debug.h>

#include <arch/riscv64.h>
#include <arch/riscv64/feature.h>
#include <ktl/byte.h>
#include <ktl/memory.h>
#include <ktl/optional.h>
#include <ktl/span.h>

// Vector register state at context switch time.
//
// We reuse zx_riscv64_thread_state_vector_regs_t out of convenience, and guard
// against unintended layout changes with the static assertions below.
using riscv64_vector_state = zx_riscv64_thread_state_vector_regs_t;

// Ensures that the above macro constants are correct.
static_assert(offsetof(riscv64_vector_state, v) == RISCV64_VECTOR_STATE_V);
static_assert(offsetof(riscv64_vector_state, vcsr) == RISCV64_VECTOR_STATE_VCSR);
static_assert(offsetof(riscv64_vector_state, vstart) == RISCV64_VECTOR_STATE_VSTART);
static_assert(offsetof(riscv64_vector_state, vl) == RISCV64_VECTOR_STATE_VL);
static_assert(offsetof(riscv64_vector_state, vtype) == RISCV64_VECTOR_STATE_VTYPE);
static_assert(sizeof(riscv64_vector_state) == 0x220);

// Read the floating point status registers.
enum class Riscv64VectorStatus { OFF, INITIAL, CLEAN, DIRTY };
inline Riscv64VectorStatus riscv64_vector_status() {
  uint64_t status = riscv64_csr_read(RISCV64_CSR_SSTATUS);

  // Convert the 2 bit field in sstatus.vs to an enum. The enum is
  // numbered the same as the raw values so the transformation should
  // just be a bitfield extraction.
  switch (status & RISCV64_CSR_SSTATUS_VS_MASK) {
    default:
    case RISCV64_CSR_SSTATUS_VS_OFF:
      return Riscv64VectorStatus::OFF;
    case RISCV64_CSR_SSTATUS_VS_INITIAL:
      return Riscv64VectorStatus::INITIAL;
    case RISCV64_CSR_SSTATUS_VS_CLEAN:
      return Riscv64VectorStatus::CLEAN;
    case RISCV64_CSR_SSTATUS_VS_DIRTY:
      return Riscv64VectorStatus::DIRTY;
  }
}

struct Thread;

extern "C" {
uint64_t rust_riscv64_vlmax(uint64_t vtype, bool* has_value);
}

inline ktl::optional<uint64_t> riscv64_vlmax(uint64_t vtype) {
  bool has_value = false;
  uint64_t val = rust_riscv64_vlmax(vtype, &has_value);
  if (!has_value) {
    return ktl::nullopt;
  }
  return val;
}

// Low-level vector context zero/save/restore routines, implemented in assembly.
extern "C" void riscv64_vector_zero();
extern "C" void riscv64_vector_save(riscv64_vector_state* state);
extern "C" void riscv64_vector_restore(const riscv64_vector_state* state);

#endif  // __ASSEMBLER__

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_VECTOR_H_
