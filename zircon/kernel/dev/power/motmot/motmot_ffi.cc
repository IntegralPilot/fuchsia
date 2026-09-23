// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/intrin.h>

#include <arch/arm64/smccc.h>
#include <arch/interrupt.h>
#include <dev/power/motmot/init.h>
#include <dev/power/motmot/motmot_ffi.h>
#include <dev/psci.h>
#include <kernel/ffi.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uint64_t cpp_motmot_modify_register_via_smc(uintptr_t phys_addr, uint32_t mask,
                                                              uint32_t val) {
  constexpr uint32_t kSmcCmdPrivReg = 0x82000504;
  constexpr uint32_t kPrivRegOptionRmw = 2;
  const arm_smccc_result_t res =
      arm_smccc_smc_internal(kSmcCmdPrivReg, phys_addr, kPrivRegOptionRmw, mask, val, 0, 0, 0);
  return res.x0;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_motmot_cpu_off_wfi_loop() {
  arch_disable_ints();
  while (true) {
    __wfi();
  }
}

}  // extern "C"
