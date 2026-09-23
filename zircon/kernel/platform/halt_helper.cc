// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/debuglog.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <zircon/boot/crash-reason.h>

#include <kernel/mp.h>
#include <platform/halt_helper.h>
#include <platform/halt_token.h>
#include <vm/pmm.h>

// Storage for the global halt token singleton
HaltToken HaltToken::g_instance;

void platform_graceful_halt_helper(platform_halt_action action, zircon_crash_reason_t reason,
                                   zx_instant_mono_t panic_deadline) {
  if (!HaltToken::Get().Take()) {
    printf("platform_graceful_halt_helper: halt/reboot already in progress; sleeping forever\n");
    Thread::Current::Sleep(ZX_TIME_INFINITE);
  }

  printf("platform_graceful_halt_helper: action=%d reason=%u panic_deadline=%ld current_time=%ld\n",
         action, static_cast<uint32_t>(reason), panic_deadline, current_mono_time());

  // Migrate to the boot CPU before shutting down the secondary CPUs.  Note that
  // this action also hard-pins our thread to the boot CPU, so we don't need to
  // worry about migration after this.
  Thread::Current::MigrateToCpu(BOOT_CPU_ID);
  printf("platform_graceful_halt_helper: Migrated thread to boot CPU.\n");

  zx_status_t status = platform_halt_secondary_cpus(panic_deadline);
  ASSERT_MSG(status == ZX_OK, "platform_halt_secondary_cpus failed: %d\n", status);
  printf("platform_graceful_halt_helper: Halted secondary CPUs.\n");

  // If we're in a debug build, check the PMM's free list for any latent corruption.
  if constexpr (LK_DEBUGLEVEL > 0) {
    pmm_checker_check_all_free_pages();
  }

  // Delay shutdown of debuglog to ensure log messages emitted by above calls will be written.
  printf("platform_graceful_halt_helper: Shutting down dlog.\n");
  status = dlog_shutdown(panic_deadline);
  ASSERT_MSG(status == ZX_OK, "dlog_shutdown failed: %d\n", status);

  printf("platform_graceful_halt_helper: Calling platform_halt.\n");
  platform_halt(action, reason);
  panic("ERROR: failed to halt the platform\n");
}

zx_status_t platform_halt_secondary_cpus(zx_instant_mono_t deadline) {
  // Ensure the current thread is pinned to the boot CPU.
  {
    Thread* current_thread = Thread::Current::Get();
    SingleChainLockGuard guard{IrqSaveOption, current_thread->get_lock(),
                               CLT_TAG("platform_halt_secondary_cpus")};
    DEBUG_ASSERT(current_thread->scheduler_state().hard_affinity() == cpu_num_to_mask(BOOT_CPU_ID));
  }

  // It's possible that the one or more secondary CPUs is still in the process
  // of booting up.  Wait for all the started CPUs to check-in before shutting
  // them down.
  //
  // The deadline here should be long enough that the secondary CPUs have had
  // ample time to startup but small enough that we don't hold up the shutdown
  // process "too long".
  zx_status_t status = mp_wait_for_all_cpus_ready(Deadline::after_mono(ZX_SEC(5)));
  if (status != ZX_OK) {
    printf("failed to wait for cpus to start: %d\n", status);
  }

  // "Unplug" online secondary CPUs before halting them.
  cpu_mask_t primary = cpu_num_to_mask(BOOT_CPU_ID);
  cpu_mask_t mask = mp_get_online_mask() & ~primary;
  status = mp_unplug_cpu_mask(mask, deadline);
  if (status == ZX_OK) {
    return ZX_OK;
  }

  // mp_unplug_cpu_mask failed.  Perhaps another thread was trying to shutdown the secondary CPUs.
  // If the primary CPU is the only one left, then we've done our job.
  if (status == ZX_ERR_BAD_STATE && mp_get_online_mask() == primary) {
    return ZX_OK;
  }

  return status;
}
