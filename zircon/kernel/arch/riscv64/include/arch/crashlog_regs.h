// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_CRASHLOG_REGS_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_CRASHLOG_REGS_H_

#include <arch/regs.h>

struct crashlog_regs_t {
  iframe_t* iframe{nullptr};

  // scause register holds the exception reason code.
  // Negative values are interrupts, positive are exceptions.
  int64_t cause{0};

  // stval register holds the faulting address in the case of a page fault
  // and (optionally) the faulting instruction in the case of an illegal
  // instruction.
  uint64_t tval{0};
};

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_CRASHLOG_REGS_H_
