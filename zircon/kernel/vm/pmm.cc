// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/pmm.h"

#include <assert.h>
#include <inttypes.h>
#include <lib/boot-options/boot-options.h>
#include <lib/console.h>
#include <lib/counters.h>
#include <lib/ktrace.h>
#include <lib/memalloc/range.h>
#include <lib/page/size.h>
#include <platform.h>
#include <pow2.h>
#include <stdlib.h>
#include <string.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <new>

#include <fbl/intrusive_double_list.h>
#include <kernel/mp.h>
#include <kernel/mutex.h>
#include <kernel/timer.h>
#include <ktl/algorithm.h>
#include <lk/init.h>
#include <vm/compression.h>
#include <vm/physmap.h>
#include <vm/pmm_arena.h>
#include <vm/pmm_checker.h>
#include <vm/pmm_node.h>
#include <vm/vm.h>

#include "vm_priv.h"

#if defined(__x86_64__)
#include <arch/x86/feature.h>
#endif

#include <ktl/enforce.h>

#define LOCAL_TRACE VM_GLOBAL_TRACE(0)

// Number of bytes available in the PMM after kernel init, but before userspace init.
KCOUNTER(boot_memory_bytes, "boot.memory.post_init_free_bytes")

// The (currently) one and only pmm node
PmmNode Pmm::node_;

// Check that if random should wait is requested that this is a debug build with assertions as it is
// currently assumed that enabling this in a non-debug build would be a mistake that should be
// caught.
static void pmm_init_alloc_random_should_wait(uint level) {
  if (BootOptions::Get()->pmm_alloc_random_should_wait) {
    ASSERT(DEBUG_ASSERT_IMPLEMENTED);
    printf("pmm: alloc-random-should-wait enabled\n");
    Pmm::Node().SeedRandomShouldWait();
  }
}
LK_INIT_HOOK(pmm_init_alloc_random_should_wait, &pmm_init_alloc_random_should_wait,
             LK_INIT_LEVEL_LAST)

static void pmm_fill_free_pages(uint level) { Pmm::Node().FillFreePagesAndArm(); }
LK_INIT_HOOK(pmm_fill, &pmm_fill_free_pages, LK_INIT_LEVEL_VM)

zx_status_t pmm_init(ktl::span<const memalloc::Range> ranges) { return Pmm::Node().Init(ranges); }

void pmm_end_handoff() { Pmm::Node().EndHandoff(); }

vm_page_t* paddr_to_vm_page(paddr_t addr) { return Pmm::Node().PaddrToPage(addr); }

size_t pmm_num_arenas() { return Pmm::Node().NumArenas(); }

zx_status_t pmm_get_arena_info(size_t count, uint64_t i, pmm_arena_info_t* buffer,
                               size_t buffer_size) {
  return Pmm::Node().GetArenaInfo(count, i, buffer, buffer_size);
}

zx_status_t pmm_alloc_page(uint alloc_flags, paddr_t* pa) {
  VM_KTRACE_DURATION(3, "pmm_alloc_page", ("count", 1), ("alloc_flags", alloc_flags));
  zx::result<vm_page_t*> result = Pmm::Node().AllocPage(alloc_flags);
  if (result.is_ok()) {
    *pa = result.value()->paddr();
  }
  return result.status_value();
}

zx_status_t pmm_alloc_page(uint alloc_flags, vm_page_t** page) {
  VM_KTRACE_DURATION(3, "pmm_alloc_page", ("count", 1), ("alloc_flags", alloc_flags));
  zx::result<vm_page_t*> result = Pmm::Node().AllocPage(alloc_flags);
  if (result.is_ok()) {
    *page = result.value();
  }
  return result.status_value();
}

zx_status_t pmm_alloc_page(uint alloc_flags, vm_page_t** page, paddr_t* pa) {
  VM_KTRACE_DURATION(3, "pmm_alloc_page", ("count", 1), ("alloc_flags", alloc_flags));
  zx::result<vm_page_t*> result = Pmm::Node().AllocPage(alloc_flags);
  if (result.is_ok()) {
    *page = result.value();
    *pa = (*page)->paddr();
  }
  return result.status_value();
}

zx_status_t pmm_alloc_pages(size_t count, uint alloc_flags, VmPageDoublyLinkedList* list) {
  VM_KTRACE_DURATION(3, "pmm_alloc_pages", ("count", count), ("alloc_flags", alloc_flags));
  return Pmm::Node().AllocPages(count, alloc_flags, list);
}

zx_status_t pmm_alloc_range(paddr_t address, size_t count, VmPageDoublyLinkedList* list) {
  VM_KTRACE_DURATION(3, "pmm_alloc_range", ("address", ktrace::Pointer{address}), ("count", count));
  return Pmm::Node().AllocRange(address, count, list);
}

zx_status_t pmm_alloc_contiguous(size_t count, uint alloc_flags, uint8_t alignment_log2,
                                 paddr_t* pa, VmPageDoublyLinkedList* list) {
  VM_KTRACE_DURATION(3, "pmm_alloc_contiguous", ("count", count), ("alloc_flags", alloc_flags));
  // if we're called with a single page, just fall through to the regular allocation routine
  if (unlikely(count == 1 && alignment_log2 <= kPageShift)) {
    zx::result<vm_page_t*> result = Pmm::Node().AllocPage(alloc_flags);
    if (result.is_ok()) {
      vm_page_t* page = result.value();
      list->push_back(page);
      *pa = page->paddr();
    }
    return result.status_value();
  }

  return Pmm::Node().AllocContiguous(count, alloc_flags, alignment_log2, pa, list);
}

void pmm_unwire_page(vm_page_t* page) { Pmm::Node().UnwirePage(page); }

void pmm_free(VmPageDoublyLinkedList* list, PmmOptDelayReuse delay_reuse) {
  VM_KTRACE_DURATION(3, "pmm_free");
  Pmm::Node().FreeList(list, delay_reuse);
}

void pmm_free_page(vm_page* page, PmmOptDelayReuse delay_reuse) {
  VM_KTRACE_DURATION(3, "pmm_free_page");
  Pmm::Node().FreePage(page, delay_reuse);
}

uint64_t pmm_count_free_pages() { return Pmm::Node().CountFreePages(); }

uint64_t pmm_count_loaned_free_pages() { return Pmm::Node().CountLoanedFreePages(); }

uint64_t pmm_count_loaned_used_pages() { return Pmm::Node().CountLoanedNotFreePages(); }

uint64_t pmm_count_loaned_pages() { return Pmm::Node().CountLoanedPages(); }

uint64_t pmm_count_loan_cancelled_pages() { return Pmm::Node().CountLoanCancelledPages(); }

uint64_t pmm_count_total_bytes() { return Pmm::Node().CountTotalBytes(); }

PageQueues* pmm_page_queues() { return Pmm::Node().GetPageQueues(); }

Evictor* pmm_evictor() { return Pmm::Node().GetEvictor(); }

bool pmm_set_free_memory_signal(uint64_t free_lower_bound, uint64_t free_upper_bound,
                                uint64_t delay_allocations_pages, Event* event) {
  return Pmm::Node().SetFreeMemorySignal(free_lower_bound, free_upper_bound,
                                         delay_allocations_pages, event);
}

void pmm_stop_returning_should_wait() { Pmm::Node().StopReturningShouldWait(); }

void pmm_checker_check_all_free_pages() { Pmm::Node().CheckAllFreePages(); }

#if __has_feature(address_sanitizer)
void pmm_asan_poison_all_free_pages() { Pmm::Node().PoisonAllFreePages(); }
#endif

static void pmm_checker_enable(size_t fill_size, CheckFailAction action) {
  // Enable filling of pages going forward.
  if (!Pmm::Node().EnableFreePageFilling(fill_size, action)) {
    printf("Checker already configured, requested fill size and action ignored.\n");
  }

  // From this point on, pages will be filled when they are freed.  However, the free list may still
  // have a lot of unfilled pages so make a pass over them and fill them all.
  Pmm::Node().FillFreePagesAndArm();

  // All free pages have now been filled with |fill_size| and the checker is armed.
}

static bool pmm_checker_is_enabled() { return Pmm::Node().Checker()->IsArmed(); }

static void pmm_checker_print_status() { Pmm::Node().Checker()->PrintStatus(stdout); }

void pmm_checker_init_from_cmdline() {
  bool enabled = false;
  switch (BootOptions::Get()->pmm_checker_enabled) {
    case CheckerEnable::kTrue:
      enabled = true;
      break;
    case CheckerEnable::kFalse:
      enabled = false;
      break;
    case CheckerEnable::kAuto:
#if defined(__x86_64__)
      if (x86_has_hypervisor()) {
        printf("PMM: Checker enabled set to auto and hypervisor detected, disabling\n");
        enabled = false;
      } else {
        printf("PMM: Checker enabled set to auto and hypervisor not detected, enabling\n");
        enabled = true;
      }
#else
      printf(
          "PMM: Checker enabled set to auto on platform without hypervisor detection, disabling\n");
      enabled = false;
#endif
      break;
  }
  if (enabled) {
    size_t fill_size = BootOptions::Get()->pmm_checker_fill_size.to_std().value_or(kPageSize);
    if (!PmmChecker::IsValidFillSize(fill_size)) {
      printf("PMM: value from %s is invalid (%lu), using kPageSize instead\n",
             kPmmCheckerFillSizeName.data(), fill_size);
      fill_size = kPageSize;
    }

    Pmm::Node().EnableFreePageFilling(fill_size, BootOptions::Get()->pmm_checker_action);
  }
}

static void pmm_dump_timer(Timer* t, zx_instant_mono_t now, void*) {
  zx_instant_mono_t deadline = zx_time_add_duration(now, ZX_SEC(1));
  t->SetOneshot(deadline, &pmm_dump_timer, nullptr);
  Pmm::Node().DumpFree();
}

LK_INIT_HOOK(
    pmm_boot_memory,
    [](unsigned int /*level*/) {
      // Track the amount of free memory available in the PMM after kernel init, but before
      // userspace starts.
      //
      // We record this in a kcounter to be tracked by build infrastructure over time.
      dprintf(INFO, "Free memory after kernel init: %" PRIu64 " bytes.\n",
              Pmm::Node().CountFreePages() * kPageSize);
      boot_memory_bytes.Set(Pmm::Node().CountFreePages() * kPageSize);
    },
    LK_INIT_LEVEL_USER - 1)

static Timer dump_free_mem_timer;

static int cmd_usage(const char* cmd_name, bool is_panic) {
  printf("usage:\n");
  printf("%s dump                                     : dump pmm info \n", cmd_name);
  if (!is_panic) {
    printf("%s free                                     : periodically dump free mem count\n",
           cmd_name);
    printf("%s drop_user_pt                             : drop all user hardware page tables\n",
           cmd_name);
    printf("%s checker status                           : prints the status of the pmm checker\n",
           cmd_name);
    printf(
        "%s checker enable [<size>] [oops|panic]     : enables the pmm checker with optional "
        "fill size and optional action\n",
        cmd_name);
    printf(
        "%s checker check                            : forces a check of all free pages in the "
        "pmm\n",
        cmd_name);
  }
  return ZX_ERR_INTERNAL;
}

static int cmd_pmm(int argc, const cmd_args* argv, uint32_t flags) {
  const bool is_panic = flags & CMD_FLAG_PANIC;
  const char* name = argv[0].str;

  if (argc < 2) {
    printf("not enough arguments\n");
    return cmd_usage(name, is_panic);
  }

  if (!strcmp(argv[1].str, "dump")) {
    Pmm::Node().Dump(is_panic);
  } else if (is_panic) {
    // No other operations will work during a panic.
    printf("Only the \"arenas\" command is available during a panic.\n");
    return cmd_usage(name, is_panic);
  } else if (!strcmp(argv[1].str, "free")) {
    static bool show_mem = false;

    if (!show_mem) {
      printf("pmm free: issue the same command to stop.\n");
      zx_instant_mono_t deadline = zx_time_add_duration(current_mono_time(), ZX_SEC(1));
      const TimerSlack slack{ZX_MSEC(20), TIMER_SLACK_CENTER};
      const Deadline slackDeadline(deadline, slack);
      dump_free_mem_timer.Set(slackDeadline, &pmm_dump_timer, nullptr);
      show_mem = true;
    } else {
      dump_free_mem_timer.Cancel();
      show_mem = false;
    }
  } else if (!strcmp(argv[1].str, "drop_user_pt")) {
    VmAspace::DropAllUserPageTables();
  } else if (!strcmp(argv[1].str, "checker")) {
    if (argc < 3 || argc > 5) {
      return cmd_usage(name, is_panic);
    }
    if (!strcmp(argv[2].str, "status")) {
      pmm_checker_print_status();
    } else if (!strcmp(argv[2].str, "enable")) {
      size_t fill_size = kPageSize;
      CheckFailAction action = PmmChecker::kDefaultAction;
      if (argc >= 4) {
        fill_size = argv[3].u;
        if (!PmmChecker::IsValidFillSize(fill_size)) {
          printf(
              "error: fill size must be a multiple of 8 and be between 8 and kPageSize, "
              "inclusive\n");
          return ZX_ERR_INTERNAL;
        }
      }
      if (argc == 5) {
        BootOptions opts;
        if (opts.Parse(argv[4].str, &BootOptions::pmm_checker_action)) {
          action = opts.pmm_checker_action;
        } else {
          printf("error: invalid action\n");
          return ZX_ERR_INTERNAL;
        }
      }
      pmm_checker_enable(fill_size, action);
      // No need to print status as enabling automatically prints status.
    } else if (!strcmp(argv[2].str, "check")) {
      if (!pmm_checker_is_enabled()) {
        printf("error: pmm checker is not enabled\n");
        return ZX_ERR_INTERNAL;
      }
      printf("checking all free pages...\n");
      pmm_checker_check_all_free_pages();
      printf("done\n");
    } else {
      return cmd_usage(name, is_panic);
    }
  } else {
    printf("unknown command\n");

    return cmd_usage(name, is_panic);
  }

  return ZX_OK;
}

void pmm_print_physical_page_borrowing_stats() {
  uint64_t free_pages = pmm_count_free_pages();
  uint64_t loaned_free_pages = pmm_count_loaned_free_pages();
  uint64_t loaned_pages = pmm_count_loaned_pages();
  uint64_t loan_cancelled_pages = pmm_count_loan_cancelled_pages();
  uint64_t total_bytes = pmm_count_total_bytes();
  uint64_t used_loaned_pages = pmm_count_loaned_used_pages();
  printf(
      "PPB stats:\n"
      "  free pages: %" PRIu64 " free MiB: %" PRIu64
      "\n"
      "  loaned free pages: %" PRIu64 " loaned free MiB: %" PRIu64
      "\n"
      "  loaned pages: %" PRIu64 " loaned MiB: %" PRIu64
      "\n"
      "  used loaned pages: %" PRIu64 " used loaned MiB: %" PRIu64
      "\n"
      "  loan cancelled pages: %" PRIu64 " loan cancelled MIB: %" PRIu64
      "\n"
      "  total physical pages: %" PRIu64 " total MiB: %" PRIu64 "\n",
      free_pages, free_pages * kPageSize / MB, loaned_free_pages,
      loaned_free_pages * kPageSize / MB, loaned_pages, loaned_pages * kPageSize / MB,
      used_loaned_pages, used_loaned_pages * kPageSize / MB, loan_cancelled_pages,
      loan_cancelled_pages * kPageSize / MB, total_bytes / kPageSize, total_bytes / MB);
}

STATIC_COMMAND_START
STATIC_COMMAND_MASKED("pmm", "physical memory manager", &cmd_pmm, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_END(pmm)
