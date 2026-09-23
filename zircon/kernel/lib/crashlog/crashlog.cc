// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/crashlog.h"

#include <ctype.h>
#include <inttypes.h>
#include <lib/affine/ratio.h>
#include <lib/boot-options/boot-options.h>
#include <lib/console.h>
#include <lib/debuglog.h>
#include <lib/io.h>
#include <lib/lockup_detector.h>
#include <platform.h>
#include <stdio.h>
#include <string-file.h>
#include <string.h>
#include <zircon/boot/crash-reason.h>
#include <zircon/time.h>

#include <arch/crashlog.h>
#include <fbl/enum_bits.h>
#include <kernel/lockdep.h>
#include <kernel/mutex.h>
#include <kernel/thread.h>
#include <ktl/algorithm.h>
#include <ktl/span.h>
#include <ktl/utility.h>
#include <object/channel_dispatcher.h>
#include <object/handle.h>
#include <object/root_job_observer.h>
#include <phys/boot-constants.h>
#include <platform/timer.h>
#include <vm/pmm.h>
#include <vm/pmm_checker.h>
#include <vm/vm.h>

#include <ktl/enforce.h>

crashlog_t g_crashlog = {};

PanicBuffer panic_buffer;

FILE stdout_panic_buffer{[](ktl::string_view str) {
  panic_buffer.Append(str);
  return gStdoutNoPersist.Write(str);
}};

namespace {

DECLARE_SINGLETON_MUTEX(RecoveredCrashlogLock);
fbl::RefPtr<VmObject> recovered_crashlog TA_GUARDED(RecoveredCrashlogLock::Get());

enum class RenderRegion : uint32_t {
  // clang-format off
  None             = 0x00,
  Banner           = 0x01,
  DebugInfo        = 0x02,
  CriticalCounters = 0x04,
  PanicBuffer      = 0x08,
  Dlog             = 0x10,
  RootJobCritical  = 0x20,
  All              = 0xffffffff,
  // clang-format on
};
FBL_ENABLE_ENUM_BITS(RenderRegion)

RenderRegion MapReasonToRegions(zircon_crash_reason_t reason) {
  switch (reason) {
    case ZirconCrashReason::NoCrash:
      return RenderRegion::None;

    case ZirconCrashReason::Oom:
      return RenderRegion::Banner | RenderRegion::CriticalCounters | RenderRegion::Dlog;

    case ZirconCrashReason::UserspaceRootJobTermination:
      return RenderRegion::Banner | RenderRegion::CriticalCounters | RenderRegion::Dlog |
             RenderRegion::RootJobCritical;

    case ZirconCrashReason::Panic:
    case ZirconCrashReason::SoftwareWatchdog:
      return RenderRegion::Banner | RenderRegion::DebugInfo | RenderRegion::CriticalCounters |
             RenderRegion::PanicBuffer | RenderRegion::Dlog;

    default:
      return RenderRegion::Banner;
  }
}

}  // namespace

size_t crashlog_to_string(ktl::span<char> target, zircon_crash_reason_t reason) {
  StringFile outfile{target};
  const RenderRegion regions = MapReasonToRegions(reason);

  if (static_cast<bool>(regions & RenderRegion::Banner)) {
    const char* reason_str;
    switch (reason) {
      case ZirconCrashReason::NoCrash:
        reason_str = "NO CRASH";
        break;

      case ZirconCrashReason::Oom:
        reason_str = "OOM";
        break;

      case ZirconCrashReason::Panic:
        reason_str = "KERNEL PANIC";
        break;

      case ZirconCrashReason::SoftwareWatchdog:
        reason_str = "SW WATCHDOG";
        break;

      case ZirconCrashReason::UserspaceRootJobTermination:
        reason_str = "USERSPACE ROOT JOB TERMINATION";
        break;

      default:
        reason_str = "UNKNOWN";
        break;
    }
    fprintf(&outfile, "ZIRCON REBOOT REASON (%s)\n\n", reason_str);

    CurrentTicksObservation ticks = timer_current_mono_and_boot_ticks();
    const affine::Ratio& transform = timer_get_ticks_to_time_ratio();

    fprintf(&outfile, "UPTIME (ms)\n%" PRIi64 "\n\n", transform.Scale(ticks.boot_now) / ZX_MSEC(1));
    fprintf(&outfile, "RUNTIME (ms)\n%" PRIi64 "\n\n",
            transform.Scale(ticks.mono_now) / ZX_MSEC(1));
    fprintf(&outfile,
            "TIMER INFO (ticks) counter:%#" PRIx64 " boot: %#" PRIx64 " mono: %#" PRIx64 "\n\n",
            // `boot_now = current_ticks + boot_ticks_offset`. The offset is negative.
            static_cast<uint64_t>(ticks.boot_now - timer_get_boot_ticks_offset()),
            static_cast<uint64_t>(timer_get_boot_ticks_offset()),
            static_cast<uint64_t>(timer_get_mono_ticks_offset()));

    outfile.Write(static_cast<bool>(regions & RenderRegion::DebugInfo)
                      // Include full symbolizer markup context for DebugInfo.
                      ? kBootConstants.kernel_debug_ident.get()
                      // Otherwise, the human-readable prefix of the full text.
                      : kBootConstants.kernel_version_ident.get());
  }

  if (static_cast<bool>(regions & RenderRegion::RootJobCritical)) {
    zx_koid_t proc_koid = RootJobObserver::GetCriticalProcessKoid();
    ktl::array proc_name_chars = RootJobObserver::GetCriticalProcessName();
    ktl::string_view proc_name(proc_name_chars.data(), proc_name_chars.size());
    proc_name = proc_name.substr(0, proc_name.find_first_of('\0'));
    if (proc_koid != ZX_KOID_INVALID) {
      fprintf(&outfile, "ROOT JOB TERMINATED BY CRITICAL PROCESS DEATH: %.*s (%" PRIu64 ")\n",
              static_cast<int>(proc_name.size()), proc_name.data(), proc_koid);
    }
  }

  if (static_cast<bool>(regions & RenderRegion::DebugInfo)) {
    // The presence of "REGISTERS" is load-bearing; we have crashlog tests that
    // that explicitly parse for it.
    // LINT.IfChange
    fprintf(&outfile, "REGISTERS\n");
    // LINT.ThenChange(
    //     //src/tests/kernel_crashlog/kernel_crashlog_test.go,
    //     //zircon/kernel/lib/crashlog/test/crashlog-tests.cc
    // )

    // Note: it is the architecture specific implementation which is responsible
    // for determining if the crashlog registers are valid, and what (if
    // anything) to print.
    arch_render_crashlog_registers(outfile, g_crashlog.regs);

    fprintf(&outfile, "BACKTRACE (up to 16 calls)\n");
    Backtrace bt;
    Thread::Current::GetBacktrace(bt);
    bt.PrintWithoutVersion(&outfile);
    fprintf(&outfile, "\n");
  }

  if (static_cast<bool>(regions & RenderRegion::CriticalCounters)) {
    // Include counters for critical events.
    fprintf(&outfile,
            "counters: haf=%" PRId64 " paf=%" PRId64 " pvf=%" PRId64 " lcs=%" PRId64 " lhb=%" PRId64
            " cf=%" PRId64 " \n",
            HandleTableArena::get_alloc_failed_count(), PmmNode::get_alloc_failed_count(),
            PmmChecker::get_validation_failed_count(), lockup_get_critical_section_oops_count(),
            lockup_get_no_heartbeat_oops_count(), ChannelDispatcher::get_channel_full_count());
  }

  if (static_cast<bool>(regions & RenderRegion::PanicBuffer)) {
    // Include as much of the contents of the panic buffer as we can, if it is
    // not empty.
    //
    // The panic buffer is one of the last thing we print.  Space is limited so
    // if the panic/assert message was long we may not be able to include the
    // whole thing.  That's OK.  The panic buffer is a "nice to have" and we've
    // already printed the primary diagnostics (register dump and backtrace).
    if (panic_buffer.size()) {
      fprintf(&outfile, "panic buffer: %s\n", panic_buffer.c_str());
    } else {
      fprintf(&outfile, "panic buffer: empty\n");
    }
  }

  if (static_cast<bool>(regions & RenderRegion::Dlog)) {
    // Finally, if we have been configured to do so, render as much of the
    // recent debug log as we can fit into the crashlog memory.

    // This banner is load-bearing server-side as it's used to extract Dlog dump.
    // Please notify //src/developer/forensics/OWNERS upon changing.
    // LINT.IfChange
    constexpr ktl::string_view kHeader{"\n--- BEGIN DLOG DUMP ---\n"};
    constexpr ktl::string_view kFooter{"\n--- END DLOG DUMP ---\n"};
    // LINT.ThenChange(//src/developer/forensics/feedback/reboot_log/reboot_log.cc)

    outfile.Write(kHeader);
    if (BootOptions::Get()->render_dlog_to_crashlog) {
      const ktl::span<char> available_region = outfile.available_region();
      const ktl::span<char> payload_region =
          available_region.size() > kFooter.size()
              ? available_region.subspan(0, available_region.size() - kFooter.size())
              : ktl::span<char>{};

      outfile.Skip(dlog_render_to_crashlog(payload_region));
    } else {
      fprintf(&outfile, "DLOG -> Crashlog disabled\n");
    }
    outfile.Write(kFooter);
  }

  return outfile.used_region().size();
}

void crashlog_stash(fbl::RefPtr<VmObject> crashlog) {
  Guard<Mutex> guard{RecoveredCrashlogLock::Get()};
  recovered_crashlog = ktl::move(crashlog);
}

fbl::RefPtr<VmObject> crashlog_get_stashed() {
  Guard<Mutex> guard{RecoveredCrashlogLock::Get()};
  return recovered_crashlog;
}

static void print_recovered_crashlog() {
  fbl::RefPtr<VmObject> crashlog = crashlog_get_stashed();
  if (!crashlog) {
    printf("no recovered crashlog\n");
    return;
  }

  // Allocate a temporary buffer so we can convert the VMO's contents to a C string.
  const size_t buffer_size = crashlog->size() + 1;
  fbl::AllocChecker ac;
  auto buffer = ktl::unique_ptr<char[]>(new (&ac) char[buffer_size]);
  if (!ac.check()) {
    printf("error: failed to allocate %lu for crashlog\n", buffer_size);
    return;
  }

  memset(buffer.get(), 0, buffer_size);
  zx_status_t status = crashlog->Read(buffer.get(), 0, buffer_size - 1);
  if (status != ZX_OK) {
    printf("error: failed to read from recovered crashlog vmo: %d\n", status);
    return;
  }

  printf("recovered crashlog follows...\n");
  printf("%s\n", buffer.get());
  printf("... end of recovered crashlog\n");
}

static int cmd_crashlog(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc < 2) {
    printf("not enough arguments\n");
  usage:
    printf("usage:\n");
    printf("%s dump                              : dump the recovered crashlog\n", argv[0].str);
    return ZX_ERR_INTERNAL;
  }

  if (!strcmp(argv[1].str, "dump")) {
    print_recovered_crashlog();
  } else {
    printf("unknown command\n");
    goto usage;
  }

  return ZX_OK;
}

STATIC_COMMAND_START
STATIC_COMMAND_MASKED("crashlog", "crashlog", &cmd_crashlog, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_END(crashlog)
