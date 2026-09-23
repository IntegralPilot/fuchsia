// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_CRASHLOG_REGS_H_
#define ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_CRASHLOG_REGS_H_

#include <stdint.h>

#include <arch/regs.h>

struct crashlog_regs_t {
  iframe_t* iframe{nullptr};

  // On x86/x64, the CR2 register is important for diagnosing kernel crashes,
  // but is not included in the iframe_t.
  uint64_t cr2{0};
  uint64_t fsbase{0};
  uint64_t gsbase{0};
  uint64_t swapgs_gsbase{0};
};

#endif  // ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_CRASHLOG_REGS_H_
