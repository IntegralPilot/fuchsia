// Copyright 2021 The Fuchsia Authors
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <debug.h>
#include <inttypes.h>
#include <lib/arch/intrin.h>
#include <lib/console.h>
#include <lib/zircon-internal/macros.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <platform.h>
#include <stdio.h>
#include <string.h>

#include <fbl/algorithm.h>
#include <kernel/event.h>
#include <ktl/iterator.h>
#include <vm/physical_page_borrowing_config.h>
#include <vm/pmm.h>

#include <ktl/enforce.h>

DECLARE_SINGLETON_MUTEX(ppb_stats_lock);
static Thread* ppb_stats_thread TA_GUARDED(ppb_stats_lock::Get()) = nullptr;
static Event ppb_stats_thread_stop_event(false);

static void cmd_ppb_borrowing_on() {
  PhysicalPageBorrowingConfig::Get().set_borrowing_on_mru_enabled(true);
  printf("borrowing enabled\n");
}

static void cmd_ppb_borrowing_off() {
  PhysicalPageBorrowingConfig::Get().set_borrowing_on_mru_enabled(false);
  printf("borrowing disabled\n");
}

static void cmd_ppb_loaning_on() {
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(true);
  printf("loaning enabled\n");
}

static void cmd_ppb_loaning_off() {
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(false);
  printf("loaning disabled\n");
}

static void cmd_ppb_stats() { pmm_print_physical_page_borrowing_stats(); }

static void cmd_ppb_stats_on() {
  Thread* local_ppb_stats_thread;
  {  // scope guard
    Guard<Mutex> guard(ppb_stats_lock::Get());
    auto stats_thread = [](void* arg) -> int {
      while (true) {
        cmd_ppb_stats();
        zx_status_t status = ppb_stats_thread_stop_event.Wait(Deadline::after_mono(ZX_SEC(1)));
        if (status == ZX_OK) {
          return 0;
        }
        DEBUG_ASSERT(status == ZX_ERR_TIMED_OUT);
      }
    };
    ppb_stats_thread = Thread::Create("ppb-stats-thread", stats_thread, nullptr, LOW_PRIORITY);
    ASSERT(ppb_stats_thread);
    local_ppb_stats_thread = ppb_stats_thread;
  }  // ~guard
  local_ppb_stats_thread->Resume();
}

static void cmd_ppb_stats_off() {
  Thread* local_ppb_stats_thread;
  {  // scope guard
    Guard<Mutex> guard(ppb_stats_lock::Get());
    local_ppb_stats_thread = ppb_stats_thread;
    ppb_stats_thread = nullptr;
  }  // ~guard
  ppb_stats_thread_stop_event.Signal();
  int retcode;
  local_ppb_stats_thread->Join(&retcode, ZX_TIME_INFINITE);
  DEBUG_ASSERT(!retcode);
  ppb_stats_thread_stop_event.Unsignal();
}

using CmdFunc = void (*)();
struct Cmd {
  const char* name;
  CmdFunc func;
};

static Cmd commands[] = {
    {.name = "borrowing_on", .func = cmd_ppb_borrowing_on},
    {.name = "borrowing_off", .func = cmd_ppb_borrowing_off},
    {.name = "loaning_on", .func = cmd_ppb_loaning_on},
    {.name = "loaning_off", .func = cmd_ppb_loaning_off},
    {.name = "stats", .func = cmd_ppb_stats},
    {.name = "stats_on", .func = cmd_ppb_stats_on},
    {.name = "stats_off", .func = cmd_ppb_stats_off},
};

// k ppb borrowing_on
//   * this is the default on boot
//   * enables page borrowing for new allocations
//   * see also k ppb borrowing_off
// k ppb borrowing_off
//   * disables page borrowing for new allocations
//   * see also k ppb borrowing_on
// k ppb loaning_on
//   * enables loaning when a contiguous VMO pages are decommitted
// k ppb loaning_off
//   * disables loaning when a contiguous VMO pages are decommitted
// k ppb stats
//   * output ppb-related stats (once)
// k ppb stats_on
//   * repeatedly output ppb-relevant stats (fairly frequently, for observing usage scenarios)
// k ppb stats_off
//   * stop repeatedly outputting ppb-relevant stats
static int cmd_ppb(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc != 2) {
    printf("2 arguments expected\n");
    printf("usage:\n");
    printf("ppb <cmd>\n");
    printf("command list:\n");
    for (auto& cmd : commands) {
      printf("%s\n", cmd.name);
    }
    return -1;
  }

  for (auto& cmd : commands) {
    if (!strcmp(argv[1].str, cmd.name)) {
      cmd.func();
      return 0;
    }
  }
  printf("sub-command not found - available sub-commands:\n");
  for (auto& cmd : commands) {
    printf("%s\n", cmd.name);
  }
  return -1;
}

STATIC_COMMAND_START
STATIC_COMMAND("ppb", "control contiguous physical page borrowing", &cmd_ppb)
STATIC_COMMAND_END(ppb)
