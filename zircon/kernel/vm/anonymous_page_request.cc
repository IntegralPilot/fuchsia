// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/anonymous_page_request.h"

#include <lib/fit/defer.h>

#include <kernel/lockdep.h>
#include <kernel/thread.h>
#include <vm/pmm.h>
#include <vm/scanner.h>
#include <vm/vm.h>

namespace {
// Atomic token used to ensure that only one thread performs the informational dump so that in the
// scenario of many threads all reaching the timeout at the same time there is not a rush of spam.
constinit ktl::atomic<bool> dump_info_before_panic_token = true;
}  // namespace

zx::result<> AnonymousPageRequest::Allocate(bool suspendable) {
  DEBUG_ASSERT(active_);
  DEBUG_ASSERT(!has_page());

  OOM_KTRACE_DURATION(1, "AnonymousPageRequest::Allocate");

  // Although the pmm_wait_till_free_pages call will unblock based on bounded kernel action, and not
  // some unbounded user request, the kernel might need to acquire arbitrary locks to achieve this.
  // Therefore blanket require no locks here to ensure no accidental lock dependencies. This can be
  // relaxed in the future if necessary.
  lockdep::AssertNoLocksHeld();

  // Time spent waiting is regarded as a memory stall.
  ScopedMemoryStall memory_stall;

  // Once we return, clear the `active_` flag.
  auto defer = fit::defer([this]() { active_ = false; });

  // This should only ever end up waiting momentarily until reclamation catches up. As such if we
  // end up waiting for a long time then this is probably a sign of a bug in reclamation somewhere,
  // so we want to make some noise here.
  constexpr zx_duration_mono_t kReportWaitTime = ZX_SEC(5);
  constexpr unsigned int kMaxWaits = ZX_SEC(30) / kReportWaitTime;
  static_assert(kMaxWaits >= 1);
  // Trigger an informational dump 1 wait prior to triggering a panic. This gives the debug log a
  // chance to get flushed out before the panic.
  constexpr unsigned int kDumpInfoWaits = kMaxWaits - 1;
  uint32_t waited = 0;

  while (true) {
    zx::result<vm_page_t*> page_alloc =
        Pmm::Node().WaitForSinglePageAllocation(Deadline::after_mono(kReportWaitTime), suspendable);
    if (page_alloc.is_error()) {
      // System is not making forward progress, lets try again.
      if (page_alloc.error_value() == ZX_ERR_TIMED_OUT) {
        waited++;
        dprintf(INFO, "WARNING: Waited %" PRIi64 " seconds to retry PMM allocations\n",
                (kReportWaitTime * waited) / ZX_SEC(1));

        // If we've reached the threshold to dump information *and* we're the first thread to do so,
        // i.e. first to get the token, then ask the scanner to do a pre panic dump. The assumption
        // here is that if we've been blocked this long then we are most likely going to hit
        // kMaxWaits and so it's worth performing a slightly unsafe informational dump for the
        // purposes of debugging.
        if (waited == kDumpInfoWaits && dump_info_before_panic_token.exchange(false)) {
          scanner_debug_dump_state_before_panic();
        }

        if (waited == kMaxWaits) {
          // If we've been waiting for a while without being able to get more memory and
          // without declaring OOM, the memory watchdog is probably wedged and the system is in an
          // unrecoverable state. It would be nice to get some diagnostics here but we don't want to
          // risk trying to acquire any locks; just panic.
          ZX_PANIC("Allocation stalled for %" PRIi64 " seconds",
                   (kReportWaitTime * waited) / ZX_SEC(1));
        }
        continue;
      }
      // The system reached an allocation allowing event, but we lost the race for the memory
      // resources. We reset the retry count, since the system IS making forward progress.
      if (page_alloc.error_value() == ZX_ERR_SHOULD_WAIT) {
        waited = 0;
        continue;
      }
      return page_alloc.take_error();
    }

    page_ = page_alloc.value();
    return zx::ok();
  }
  __UNREACHABLE;
}

void AnonymousPageRequest::FreePage() { Pmm::Node().FreePage(take_page()); }
