// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_FPU_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_FPU_H_

#include <stdint.h>

#include <arch/riscv64.h>

// Double precision floating point state at context switch time.
struct riscv64_fpu_state {
  uint64_t f[32];
  uint32_t fcsr;
};

// Low level fpu context zero/save/restore routines, implemented in assembly.
extern "C" void riscv64_fpu_zero();
extern "C" void riscv64_fpu_save(riscv64_fpu_state* state);
extern "C" void riscv64_fpu_restore(const riscv64_fpu_state* state);

// Read the floating point status registers
enum class Riscv64FpuStatus : uint64_t { OFF, INITIAL, CLEAN, DIRTY };
inline Riscv64FpuStatus riscv64_fpu_status() {
  uint64_t status = riscv64_csr_read(RISCV64_CSR_SSTATUS);

  // Convert the 2 bit field in sstatus.fs to an enum. The enum is
  // numbered the same as the raw values so the transformation should
  // just be a bitfield extraction.
  switch (status & RISCV64_CSR_SSTATUS_FS_MASK) {
    default:
    case RISCV64_CSR_SSTATUS_FS_OFF:
      return Riscv64FpuStatus::OFF;
    case RISCV64_CSR_SSTATUS_FS_INITIAL:
      return Riscv64FpuStatus::INITIAL;
    case RISCV64_CSR_SSTATUS_FS_CLEAN:
      return Riscv64FpuStatus::CLEAN;
    case RISCV64_CSR_SSTATUS_FS_DIRTY:
      return Riscv64FpuStatus::DIRTY;
  }
}

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_FPU_H_
