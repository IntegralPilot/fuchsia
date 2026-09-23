// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/console.h>
#include <lib/page/size.h>

#include <object/memory_watchdog.h>
#include <vm/pmm.h>
#include <vm/scanner.h>

// Allocate memory until the PMM returns ZX_ERR_NO_MEMORY, then free it all.
//
// The idea is to cause the system to briefly dip into an out of memory state.
static int cmd_oom_dip() {
  VmPageDoublyLinkedList list;
  while (pmm_alloc_pages(1, 0, &list) != ZX_ERR_NO_MEMORY) {
  }
  pmm_free(&list);
  printf("allocated until ZX_ERR_NO_MEMORY (and then freed)\n");
  return ZX_OK;
}

static int cmd_oom(int argc, const cmd_args* argv) {
  uint64_t rate = 0;
  bool hard = false;
  if (argc > 2) {
    if (!strcmp(argv[2].str, "dip")) {
      return cmd_oom_dip();
    }

    if (!strcmp(argv[2].str, "hard")) {
      hard = true;
    } else {
      rate = strtoul(argv[2].str, nullptr, 0) * 1024 * 1024 / kPageSize;
    }
  }

  // When we reach the oom state the kernel may 'try harder' to reclaim memory and prevent us from
  // hitting OOM. To avoid this we disable the scanner (and evictor) to prevent additional memory
  // from becoming classified as evictable, and then evict anything that is already considered.
  printf("Disabling VM scanner\n");
  scanner_push_disable_count();
  uint64_t pages_evicted =
      pmm_evictor()
          ->EvictSynchronous(/*min_mem_to_free=*/UINT64_MAX, /*free_mem_target=*/0,
                             Evictor::EvictionLevel::IncludeNewest, Evictor::Output::NoPrint)
          .counts.non_loaned_total();
  if (pages_evicted > 0) {
    printf("Leaked %" PRIu64 " pages from eviction\n", pages_evicted);
  }

  uint64_t pages_till_oom;
  // In case we are racing with someone freeing pages we will leak in a loop until we are sure
  // we have hit the oom state.
  while ((pages_till_oom = GetMemoryWatchdog().DebugNumBytesTillPressureLevel(
                               MemoryWatchdog::PressureLevel::kOutOfMemory) /
                           kPageSize) > 0) {
    VmPageDoublyLinkedList list;
    if (rate > 0) {
      uint64_t pages_leaked = 0;
      while (pages_leaked < pages_till_oom) {
        uint64_t alloc_pages = ktl::min(rate, pages_till_oom - pages_leaked);
        if (pmm_alloc_pages(alloc_pages, 0, &list) == ZX_OK) {
          pages_leaked += alloc_pages;
          printf("Leaked %lu pages\n", pages_leaked);
        }
        Thread::Current::SleepRelative(ZX_SEC(1));
      }
    } else {
      if (pmm_alloc_pages(pages_till_oom, 0, &list) == ZX_OK) {
        printf("Leaked %lu pages\n", pages_till_oom);
      }
    }
    // We are deliberately leaking the allocated pages, so clear the list structure
    // without freeing the pages to avoid the empty assertion.
    list.clear();
    // Ignore any errors under the assumption we had a racy allocation and try again next time
    // around the loop.
  }

  if (hard) {
    printf("Continuing to leak pages forever\n");
    // Keep leaking as fast possible.
    while (true) {
      vm_page_t* p;
      pmm_alloc_page(0, &p);
    }
  }

  return ZX_OK;
}

static int cmd_usage(const char* cmd_name) {
  printf("usage:\n");
  printf("%s dump                                     : dump memory availability state info \n",
         cmd_name);
  printf(
      "%s oom [<rate>]                             : leak memory until OOM is triggered, "
      "optionally specify the rate at which to leak (in MB per second). "
      "This tries to reliably get the system into OOM state, so might disable some kinds of "
      "kernel reclamation that prevent going into OOM. If you want to test system response "
      "to memory pressure, unaltered from the default behavior, use avail_state instead.\n",
      cmd_name);
  printf("%s oom dip                                  : allocate until no mem, then free\n",
         cmd_name);
  printf(
      "%s oom hard                                 : leak memory aggressively and keep on "
      "leaking\n",
      cmd_name);
  printf(
      "%s avail_state [step] <state> [<nsecs>] : allocate memory to go to memstate "
      "<state>, hold the state for <nsecs> (10s by default). Only works if going to <state> "
      "from current state requires allocating memory, can't free up pre-allocated memory. In "
      "optional [step] mode, allocation pauses for 1 second at each intermediate memory "
      "availability state until <state> is reached.\n",
      cmd_name);
  return ZX_ERR_INTERNAL;
}

static int cmd_mem(int argc, const cmd_args* argv, uint32_t flags) {
  const char* name = argv[0].str;

  if (argc < 2) {
    printf("not enough arguments\n");
    return cmd_usage(name);
  }

  if (!strcmp(argv[1].str, "dump")) {
    GetMemoryWatchdog().Dump();
  } else if (!strcmp(argv[1].str, "oom")) {
    if (argc > 3) {
      return cmd_usage(name);
    }
    return cmd_oom(argc, argv);
  } else if (!strcmp(argv[1].str, "avail_state")) {
    if (argc < 3) {
      return cmd_usage(name);
    }
    bool step = false;
    int index = 2;
    if (!strcmp(argv[2].str, "step")) {
      step = true;
      index++;
    }

    MemoryWatchdog::PressureLevel state =
        static_cast<MemoryWatchdog::PressureLevel>(argv[index++].u);
    if (state >= MemoryWatchdog::PressureLevel::kNumLevels) {
      printf("Invalid memstate %u. Specify a value between 0 and %u.\n", state,
             MemoryWatchdog::PressureLevel::kNumLevels - 1u);
      return cmd_usage(name);
    }

    uint64_t pages_to_alloc, pages_to_free = 0;
    VmPageDoublyLinkedList list;

    if (step) {
      uint8_t s = MemoryWatchdog::PressureLevel::kNumLevels - 1;
      while (true) {
        // In case we are racing with someone freeing pages we will leak in a loop until we are
        // sure we have hit the required memory availability state.
        uint64_t pages_allocated = 0;
        while ((pages_to_alloc = GetMemoryWatchdog().DebugNumBytesTillPressureLevel(
                                     MemoryWatchdog::PressureLevel(s)) /
                                 kPageSize) > 0) {
          if (pmm_alloc_pages(pages_to_alloc, 0, &list) == ZX_OK) {
            printf("Leaked %lu pages\n", pages_to_alloc);
            pages_allocated += pages_to_alloc;
          }
        }
        pages_to_free += pages_allocated;
        if (s == state) {
          break;
        }
        s--;
        if (pages_allocated) {
          printf("Sleeping for 1 second...\n");
          Thread::Current::SleepRelative(ZX_SEC(1));
        }
      }
    } else {
      // In case we are racing with someone freeing pages we will leak in a loop until we are
      // sure we have hit the required memory availability state.
      while ((pages_to_alloc =
                  GetMemoryWatchdog().DebugNumBytesTillPressureLevel(state) / kPageSize) > 0) {
        if (pmm_alloc_pages(pages_to_alloc, 0, &list) == ZX_OK) {
          printf("Leaked %lu pages\n", pages_to_alloc);
          pages_to_free += pages_to_alloc;
        }
      }
    }

    if (pages_to_free > 0) {
      uint64_t nsecs = 10;
      if (argc > index) {
        nsecs = argv[index].u;
      }
      printf("Sleeping for %lu seconds...\n", nsecs);
      Thread::Current::SleepRelative(ZX_SEC(nsecs));
      pmm_free(&list);
      printf("Freed %lu pages\n", pages_to_free);
    }
  } else {
    printf("unknown command\n");

    return cmd_usage(name);
  }

  return ZX_OK;
}

STATIC_COMMAND_START
STATIC_COMMAND_MASKED("mem", "memory commands", &cmd_mem, CMD_AVAIL_NORMAL)
STATIC_COMMAND_END(mem_cmd)
