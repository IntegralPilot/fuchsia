// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <platform.h>

#include <arch/riscv64/feature.h>
#include <dev/timer.h>
#include <platform/timer.h>

template <GetTicksSyncFlag Flags>
inline zx_ticks_t platform_current_raw_ticks_synchronized() {
  // If the caller requested that the read of the current raw ticks be synchronized with respect to
  // previous loads and/or stores, ensure that we emit a fence instruction guaranteeing this as
  // described in section 6.1.1 ("CSR Access Ordering") in "The RISC-V Instruction Set Manual,
  // Volume I, Unprivileged Architecture" Version 20250508. The specification provides more detail
  // on this approach, but in summary:
  // * The timer is read via a CSR, which is modeled as a device input operation in the RISC-V
  //   memory model. As such, the successor set must consist of the "i" operand.
  // * Depending on the GetTicksSyncFlag, the predecessor set must be "r", "w", or "rw".
  constexpr GetTicksSyncFlag AfterPreviousLoadsAndStores =
      GetTicksSyncFlag::kAfterPreviousLoads | GetTicksSyncFlag::kAfterPreviousStores;
  if constexpr ((Flags & AfterPreviousLoadsAndStores) == AfterPreviousLoadsAndStores) {
    __asm__ volatile("fence rw, i" ::: "memory");
  } else if constexpr ((Flags & GetTicksSyncFlag::kAfterPreviousLoads) != GetTicksSyncFlag::kNone) {
    __asm__ volatile("fence r, i" ::: "memory");
  } else if constexpr ((Flags & GetTicksSyncFlag::kAfterPreviousStores) !=
                       GetTicksSyncFlag::kNone) {
    __asm__ volatile("fence w, i" ::: "memory");
  }

  // Redirect the current ticks call to the pdev layer.
  const zx_ticks_t ticks = timer_current_ticks();

  // If the caller requested that the read of the current raw ticks be synchronized with respect
  // to subsequent loads and/or stores, then once again ensure that we emit a fence instruction.
  // Once again, timer reads are modeled as a device input operation, so:
  // * The predecessor set must consist of the "i" operand.
  // * Depending on the GetTicksSyncFlag, the successor set must be "r", "w", or "rw".
  constexpr GetTicksSyncFlag BeforeSubsequentLoadsAndStores =
      GetTicksSyncFlag::kBeforeSubsequentLoads | GetTicksSyncFlag::kBeforeSubsequentStores;
  if constexpr ((Flags & BeforeSubsequentLoadsAndStores) == BeforeSubsequentLoadsAndStores) {
    __asm__ volatile("fence i, rw" ::: "memory");
  } else if constexpr ((Flags & GetTicksSyncFlag::kBeforeSubsequentLoads) !=
                       GetTicksSyncFlag::kNone) {
    __asm__ volatile("fence i, r" ::: "memory");
  } else if constexpr ((Flags & GetTicksSyncFlag::kBeforeSubsequentStores) !=
                       GetTicksSyncFlag::kNone) {
    __asm__ volatile("fence i, w" ::: "memory");
  }
  return ticks;
}

zx_instant_mono_ticks_t platform_convert_early_ticks(arch::EarlyTicks sample) {
  return sample.time + timer_get_mono_ticks_offset();
}

zx_status_t platform_set_oneshot_timer(zx_ticks_t deadline) {
  return timer_set_oneshot_timer(deadline);
}

void platform_stop_timer() { timer_stop(); }

void platform_shutdown_timer() { timer_shutdown(); }

zx_status_t platform_suspend_timer_curr_cpu() { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t platform_resume_timer_curr_cpu() { return ZX_ERR_NOT_SUPPORTED; }

bool platform_usermode_can_access_tick_registers() {
  // If the cpu claims to have Zicntr support, then it's relatively cheap for user
  // space to access the time CSR via rdtime instruction.
  return riscv64_feature_has_zicntr();
}

// Explicit instantiation of all of the forms of synchronized tick access.
#define EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(flags) \
  template zx_ticks_t                                         \
  platform_current_raw_ticks_synchronized<static_cast<GetTicksSyncFlag>(flags)>()
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(0);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(1);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(2);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(3);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(4);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(5);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(6);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(7);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(8);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(9);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(10);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(11);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(12);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(13);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(14);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(15);
#undef EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED
