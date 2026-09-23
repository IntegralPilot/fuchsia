// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_CRASHLOG_REGS_H_
#define ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_CRASHLOG_REGS_H_

#include <stdint.h>

#include <arch/regs.h>

struct crashlog_regs_t {
  iframe_t* iframe{nullptr};

  // On arm64, the ESR and FAR are important for diagnosing kernel crashes, but
  // are not included in the iframe_t.
  uint32_t esr{0};
  uint64_t far{0};

  uint64_t tpidr_el0{};
  uint64_t tpidr_el1{};
  uint64_t tpidrro_el0{};
};

#endif  // ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_CRASHLOG_REGS_H_
