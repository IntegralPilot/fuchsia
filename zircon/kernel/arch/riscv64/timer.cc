// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/affine/ratio.h>
#include <lib/zbi-format/driver-config.h>
#include <platform.h>
#include <trace.h>
#include <zircon/types.h>

#include <arch/riscv64/timer.h>
#include <kernel/ffi.h>
#include <pdev/timer.h>
#include <platform/timer.h>

extern "C" {

zx_ticks_t rust_riscv_sbi_current_ticks();
zx_status_t rust_riscv_sbi_set_oneshot_timer(zx_ticks_t deadline);
zx_status_t rust_riscv_sbi_timer_stop();
zx_status_t rust_riscv_sbi_timer_shutdown();
FFI_ALWAYS_INLINE void cpp_timer_tick() { timer_tick(); }

FFI_ALWAYS_INLINE void cpp_timer_set_conversion_and_register(uint32_t cntfrq,
                                                             uint64_t initial_ticks) {
  affine::Ratio cntpct_to_nsec = {ZX_SEC(1), cntfrq};
  dprintf(SPEW, "riscv generic timer cntpct_per_nsec: %u/%u\n", cntpct_to_nsec.numerator(),
          cntpct_to_nsec.denominator());
  timer_set_ticks_to_time_ratio(cntpct_to_nsec);
  timer_set_initial_ticks(initial_ticks);

  static const pdev_timer_ops riscv_sbi_timer_ops = {
      .current_ticks = rust_riscv_sbi_current_ticks,
      .set_oneshot_timer = rust_riscv_sbi_set_oneshot_timer,
      .stop = rust_riscv_sbi_timer_stop,
      .shutdown = rust_riscv_sbi_timer_shutdown,
  };
  pdev_register_timer(&riscv_sbi_timer_ops);
}

}  // extern "C"
