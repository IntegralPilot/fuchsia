// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "arch/riscv64/sbi.h"

#include <lib/arch/riscv64/sbi.h>

#include <arch/riscv64/mp.h>
#include <kernel/ffi.h>
#include <pdev/power.h>

extern "C" {
FFI_ALWAYS_INLINE void cpp_pdev_register_sbi_power() {
  static const pdev_power_ops sbi_ops = {
      .reboot = [](power_reboot_flags flags) -> zx_status_t { return sbi_reset(); },
      .shutdown = sbi_shutdown,
      .cpu_off = sbi_hart_stop,
      .cpu_on = sbi_hart_start,
      .get_cpu_state = sbi_get_cpu_state,
  };
  pdev_register_power(&sbi_ops);
}
}  // extern "C"
