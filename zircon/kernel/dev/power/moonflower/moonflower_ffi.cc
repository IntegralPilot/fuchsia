// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/ddk/platform-defs.h>
#include <lib/power-management/energy-model.h>
#include <lib/power-management/pdev-power-level-controller.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/arm64/periphmap.h>
#include <arch/arm64/smccc.h>
#include <dev/power/moonflower/init.h>
#include <dev/power/moonflower/moonflower_ffi.h>
#include <dev/psci.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <kernel/scheduler.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uintptr_t cpp_moonflower_get_opp_vaddr() {
  return reinterpret_cast<uintptr_t>(periph_paddr_to_vaddr(0xf521000));
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE int64_t cpp_moonflower_tz_config_hw_for_ram_dump(uint64_t disable_wd_dbg,
                                                                   uint64_t boot_partition_sel) {
  constexpr uint32_t kTzConfigHwForRamDumpFuncId = 0xc2000109;
  constexpr uint32_t kTzConfigHwForRamDumpParamId = 0x2;
  arm_smccc_result_t res = arm_smccc_smc(kTzConfigHwForRamDumpFuncId, kTzConfigHwForRamDumpParamId,
                                         disable_wd_dbg, boot_partition_sel, 0, 0, 0, 0);
  return static_cast<int64_t>(res.x0);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE int64_t cpp_moonflower_tz_io_write(paddr_t paddr, uint32_t val) {
  constexpr uint32_t kTzIoAccessWriteFuncId = 0xc2000502;
  constexpr uint32_t kTzIoAccessWriteParamId = 0x2;
  arm_smccc_result_t res =
      arm_smccc_smc(kTzIoAccessWriteFuncId, kTzIoAccessWriteParamId, paddr, val, 0, 0, 0, 0);
  return static_cast<int64_t>(res.x0);
}

}  // extern "C"
