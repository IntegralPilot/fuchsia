// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/lockup_detector/diagnostics.h"

#include <debug.h>
#include <inttypes.h>
#include <lib/boot-options/boot-options.h>
#include <lib/page/size.h>

#include <kernel/scheduler.h>
#include <ktl/bit.h>
#include <object/process_dispatcher.h>
#include <object/thread_dispatcher.h>

#if defined(__aarch64__)
#include <arch/arm64/dap.h>
#include <arch/arm64/mmu.h>
#endif

#if defined(__x86_64__)
#include <lib/backtrace/global_cpu_context_exchange.h>
#endif

#include <ktl/enforce.h>

#if defined(__aarch64__)

bool CanDumpRegistersAndBacktrace() { return arm64_dap_is_enabled(); }

zx_status_t DumpRegistersAndBacktrace(cpu_num_t cpu, FILE* output_target) {
  if (!CanDumpRegistersAndBacktrace()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  arm64_dap_processor_state state;
  // TODO(maniscalco): Update the DAP to make use of lockup_detector_diagnostic_query_timeout_ms.
  zx_status_t result = arm64_dap_read_processor_state(cpu, &state);

  if (result != ZX_OK) {
    fprintf(output_target, "Failed to read DAP state (res %d)\n", result);
    return result;
  }

  fprintf(output_target, "DAP state:\n");
  state.Dump(output_target);
  fprintf(output_target, "\n");

  if constexpr (__has_feature(shadow_call_stack)) {
    Backtrace bt;
    zx_status_t status = lockup_internal::GetBacktraceFromDapState(state, bt);
    switch (status) {
      case ZX_ERR_BAD_STATE:
        fprintf(output_target, "DAP backtrace: CPU-%u not in kernel mode.\n", cpu);
        break;
      case ZX_ERR_INVALID_ARGS:
        fprintf(output_target, "DAP backtrace: invalid SCSP.\n");
        break;
      case ZX_ERR_OUT_OF_RANGE:
        fprintf(output_target, "DAP backtrace: not a kernel address.\n");
        break;
      case ZX_ERR_NOT_FOUND:
        fprintf(output_target, "DAP backtrace: not mapped.\n");
        break;
      default:
        fprintf(output_target, "DAP backtrace: %d\n", status);
    }
    if (bt.size() > 0) {
      bt.PrintWithoutVersion(output_target);
    }
    return status;
  } else {
    return ZX_OK;
  }
}

#elif defined(__x86_64__)

namespace {

// A value <= 0 means the diagnostic query is disabled.
zx_duration_t GetTimeout() {
  return ZX_MSEC(BootOptions::Get()->lockup_detector_diagnostic_query_timeout_ms);
}

}  // namespace

bool CanDumpRegistersAndBacktrace() { return (GetTimeout() > 0); }

zx_status_t DumpRegistersAndBacktrace(cpu_num_t cpu, FILE* output_target) {
  zx_duration_t timeout = GetTimeout();
  if (timeout == 0) {
    fprintf(output_target, "diagnostic query disabled (timeout is 0)\n");
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Make sure interrupts are disabled before calling |RequestContext|.
  InterruptDisableGuard irqd;

  // First, dump the context for the unresponsive CPU.  Then, dump the contexts of the other CPUs.
  cpu_num_t target_cpu = cpu;
  cpu_mask_t remaining_cpus = Scheduler::PeekActiveMask() & ~cpu_num_to_mask(target_cpu);
  zx_status_t result = ZX_OK;
  do {
    CpuContext context;
    zx_status_t status = g_cpu_context_exchange.RequestContext(target_cpu, timeout, context);
    if (status != ZX_OK) {
      fprintf(output_target, "failed to get context of CPU-%u: %d\n", target_cpu, status);
      if (result == ZX_OK) {
        result = status;
      }
    } else {
      fprintf(output_target, "CPU-%u context follows\n", target_cpu);
      context.backtrace.PrintWithoutVersion(output_target);
      PrintFrame(context.frame, output_target);
      fprintf(output_target, "end of CPU-%u context\n", target_cpu);
    }
  } while ((target_cpu = remove_cpu_from_mask(remaining_cpus)) != INVALID_CPU);

  return result;
}

#elif defined(__riscv)

bool CanDumpRegistersAndBacktrace() { return false; }

zx_status_t DumpRegistersAndBacktrace(cpu_num_t cpu, FILE* output_target) {
  return ZX_ERR_NOT_SUPPORTED;
}

#else
#error "Unknown architecture!"
#endif

namespace lockup_internal {

void DumpCommonDiagnostics(cpu_num_t cpu, FILE* output_target, FailureSeverity severity) {
  DEBUG_ASSERT(arch_ints_disabled());

  auto& percpu = percpu::Get(cpu);
  fprintf(output_target, "timer_ints: %lu, interrupts: %lu\n", percpu.stats.timer_ints,
          percpu.stats.interrupts);

  percpu.scheduler.Dump(output_target);
  percpu.scheduler.DumpActiveThread(output_target);

  if (severity == FailureSeverity::Fatal) {
    fprintf(output_target, "\n");
    zx_status_t status = DumpRegistersAndBacktrace(cpu, output_target);
    if (status != ZX_OK) {
      fprintf(output_target, "failed to dump registers and backtrace: %d\n", status);
    }
  }
}

#if defined(__aarch64__)
zx_status_t GetBacktraceFromDapState(const arm64_dap_processor_state& state, Backtrace& out_bt) {
  // Don't attempt to do any backtracing unless this looks like the thread is in the kernel right
  // now.  The PC might be completely bogus, but even if it is in a legit user mode process, I'm not
  // sure of a good way to print the symbolizer context for that process, or to figure out if the
  // process is using a shadow call stack or not.
  if (state.get_el_level() != 1u) {
    return ZX_ERR_BAD_STATE;
  }

  // Build a backtrace using the PC as frame 0's address and the LR as frame 1's address.
  out_bt.reset();
  out_bt.push_back(state.pc);
  out_bt.set_first_frame_type(Backtrace::FrameType::PreciseLocation);
  out_bt.push_back(state.r[30]);

  // Is the Shadow Call Stack Pointer (x18) properly aligned?
  constexpr size_t kPtrSize = sizeof(void*);
  uintptr_t scsp = state.r[18];
  if (scsp & (kPtrSize - 1)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // SCSP has post-increment semantics so back up one slot so that it points to a stored value.
  if (scsp == 0) {
    return ZX_ERR_INVALID_ARGS;
  }
  scsp -= kPtrSize;

  // Is the address in the kernel's address space?
  if (!is_kernel_address(scsp)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // And is it mapped?
  paddr_t pa_unused;
  zx_status_t status = arm64_mmu_translate(scsp, &pa_unused, /*user=*/false, /*write=*/false);
  if (status != ZX_OK) {
    return status;
  }

  // The SCSP looks legit.  Copy the return address values, but don't cross a page boundary.
  while (out_bt.size() < Backtrace::kMaxSize) {
    vaddr_t ret_addr = *reinterpret_cast<vaddr_t*>(scsp);
    out_bt.push_back(ret_addr);

    // Are we about to cross a page boundary?
    static_assert(ktl::has_single_bit(static_cast<uint64_t>(kPageSize)),
                  "kPageSize is not a power of 2!  Wut??");
    if ((scsp & (kPageSize - 1)) == 0) {
      break;
    }
    scsp -= kPtrSize;
  }

  return ZX_OK;
}
#endif  // __aarch64__

}  // namespace lockup_internal
