// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_LOCKUP_DETECTOR_INCLUDE_LIB_LOCKUP_DETECTOR_DIAGNOSTICS_H_
#define ZIRCON_KERNEL_LIB_LOCKUP_DETECTOR_INCLUDE_LIB_LOCKUP_DETECTOR_DIAGNOSTICS_H_

#include <inttypes.h>
#include <stdio.h>

#include <arch/vm.h>
#include <kernel/cpu.h>

#if defined(__aarch64__)
#include <arch/arm64/dap.h>
#include <arch/arm64/mmu.h>
#endif

// Returns true if the system has the ability to dump the state of another CPU.
//
// Intended to be used in conjunction with |DumpRegistersAndBacktrace|.
//
// If this function returns false, |DumpRegistersAndBacktrace| should not be called.  Of course,
// even if this function returns true, |DumpRegistersAndBacktrace| may still fail.
bool CanDumpRegistersAndBacktrace();

// Attempt to dump the registers and backtrace of the target |cpu|.
//
// This is a destructive operation and may leave the target CPU is an unknown state.  It should only
// be called as part of a panic.
//
//
// Example:
//
// if (CanDumpRegistersAndBacktrace()) {
//   // Initiate a panic sequence to ensure that subsequent printfs are sent to serial,
//   // however, don't try to halt any CPUs because we want to query the target's state.
//   platform_panic_start(PanicStartHaltOtherCpus::No);
//   printf("cpu-%d is unresponsive, attempting to dump its state", target_cpu);
//
//   // Attempt to dump state.
//   zx_status_t status = DumpRegistersAndBacktrace(target_cpu, stdout);
//   if (status != ZX_OK) {
//     printf("failed to dump state for cpu-%d, status %d\n", target_cpu, status);
//   }
//   platform_halt(HALT_ACTION_HALT, ZirconCrashReason::Panic);
// }
//
//
// Errors (non-exhaustive list):
//
//   ZX_ERR_NOT_SUPPORTED - there is no debug facility present to support this.
//   ZX_ERR_TIMED_OUT - timed out trying to get CPU context.
zx_status_t DumpRegistersAndBacktrace(cpu_num_t cpu, FILE* output_target);

// This namespace is logically private to lockup_detector and its tests.
namespace lockup_internal {

enum class FailureSeverity { Oops, Fatal };
void DumpCommonDiagnostics(cpu_num_t cpu, FILE* output_target, FailureSeverity severity);

#if defined(__aarch64__)
// Using the supplied DAP state, obtain a backtrace, taking care to not fault.
//
// This function is exposed in a header file to facilitate testing.  See |DumpRegistersAndBacktrace|
// for the platform and architecture independent interface.
//
// Resets |out_bt| and then fills it in as much as possible.  The backtrace may be truncated if the
// SCS crosses a page boundary.  The contents of |out_bt| are valid even on error.
//
// Errors:
//   ZX_ERR_BAD_STATE - if the CPU is not in kernel mode.
//   ZX_ERR_INVALID_ARGS - if the SCSP pointer is null or unaligned.
//   ZX_ERR_OUT_OF_RANGE - if the stack is outside kernel address space.
//   ZX_ERR_NOT_FOUND - if the stack is not mapped.
zx_status_t GetBacktraceFromDapState(const arm64_dap_processor_state& state, Backtrace& out_bt);
#endif  // __aarch64__

}  // namespace lockup_internal

#endif  // ZIRCON_KERNEL_LIB_LOCKUP_DETECTOR_INCLUDE_LIB_LOCKUP_DETECTOR_DIAGNOSTICS_H_
