// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ARCH_INTERRUPT_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ARCH_INTERRUPT_H_

// Implementation of riscv64 specific routines to disable and reenable
// local interrupts on the current CPU.

#ifndef __ASSEMBLER__

#include <stdint.h>

#include <arch/riscv64.h>
#include <ktl/atomic.h>

using interrupt_saved_state_t = bool;

constexpr interrupt_saved_state_t kNoopInterruptSavedState = false;

[[nodiscard]] inline interrupt_saved_state_t arch_interrupt_save() {
  interrupt_saved_state_t old_state;
  old_state = riscv64_csr_read_clear(RISCV64_CSR_SSTATUS, RISCV64_CSR_SSTATUS_SIE) &
              RISCV64_CSR_SSTATUS_SIE;
  if (old_state) {
    ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
  }
  return old_state;
}

static inline void arch_interrupt_restore(interrupt_saved_state_t old_state) {
  riscv64_csr_set(RISCV64_CSR_SSTATUS, old_state ? RISCV64_CSR_SSTATUS_SIE : 0);
}

inline void arch_disable_ints() {
  riscv64_csr_clear(RISCV64_CSR_SSTATUS, RISCV64_CSR_SSTATUS_SIE);
  ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
}

inline void arch_enable_ints() {
  ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
  riscv64_csr_set(RISCV64_CSR_SSTATUS, RISCV64_CSR_SSTATUS_SIE);
}

inline bool arch_ints_disabled() {
  return (riscv64_csr_read(RISCV64_CSR_SSTATUS) & RISCV64_CSR_SSTATUS_SIE) == 0;
}

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_ARCH_INTERRUPT_H_
