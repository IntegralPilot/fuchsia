// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <debug.h>
#include <lib/arch/asm.h>
#include <lib/boot-options/boot-options.h>
#include <lib/debuglog.h>
#include <lib/fit/defer.h>
#include <lib/instrumentation/asan.h>
#include <lib/page/size.h>
#include <lib/syscalls/forward.h>
#include <lib/wake-vector.h>
#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/checking.h>
#include <lib/zbitl/view.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <string.h>
#include <sys/types.h>
#include <trace.h>
#include <zircon/boot/crash-reason.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/syscalls/resource.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstdio>

#include <arch/arch_ops.h>
#include <arch/mp.h>
#include <arch/ops.h>
#include <dev/hw_watchdog.h>
#include <dev/interrupt.h>
#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/cpu.h>
#include <kernel/ffi.h>
#include <kernel/idle_power_thread.h>
#include <kernel/mp.h>
#include <kernel/percpu.h>
#include <kernel/range_check.h>
#include <kernel/scheduler.h>
#include <kernel/thread.h>
#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/unique_ptr.h>
#include <object/event_dispatcher.h>
#include <object/handle.h>
#include <object/job_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/vm_object_dispatcher.h>
#include <phys/handoff.h>
#include <platform/halt_helper.h>
#include <platform/halt_token.h>
#include <platform/mexec.h>
#include <vm/handoff-end.h>
#include <vm/physmap.h>
#include <vm/pmm.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>

#include "system_priv.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

extern "C" zx_status_t cpp_system_mexec_payload_get_helper(uint8_t* buffer, size_t buffer_size,
                                                           size_t* out_zbi_size);

extern "C" NO_ASAN zx_status_t cpp_system_mexec_core(zx_handle_t resource, zx_handle_t kernel_vmo,
                                                     zx_handle_t data_zbi_vmo);

extern "C" zx_status_t cpp_system_mexec_payload_get_helper(uint8_t* buffer, size_t buffer_size,
                                                           size_t* out_zbi_size) {
  if (auto result = WriteMexecData({reinterpret_cast<ktl::byte*>(buffer), buffer_size});
      result.is_error()) {
    return result.error_value();
  } else {
    *out_zbi_size = ktl::move(result).value();
    return ZX_OK;
  }
}

extern "C" NO_ASAN zx_status_t cpp_system_mexec_core(zx_handle_t resource, zx_handle_t kernel_vmo,
                                                     zx_handle_t data_zbi_vmo) {
  return system_mexec_core(resource, kernel_vmo, data_zbi_vmo);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_scheduler_update_processing_rates(
    zx_cpu_performance_info_t* info, size_t count) {
  Scheduler::UpdateProcessingRates(ktl::span{info, count});
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_scheduler_update_processing_limits(zx_cpu_perf_limit_t* info,
                                                                         size_t count) {
  Scheduler::UpdateProcessingLimits(ktl::span{info, count});
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_scheduler_get_performance_scales(
    zx_cpu_performance_info_t* info, size_t count) {
  Scheduler::GetPerformanceScales(ktl::span{info, count});
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_scheduler_get_default_performance_scales(
    zx_cpu_performance_info_t* info, size_t count) {
  Scheduler::GetDefaultPerformanceScales(ktl::span{info, count});
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_scheduler_get_processing_limits(zx_cpu_perf_limit_t* info,
                                                                      size_t count) {
  Scheduler::GetProcessingLimits(ktl::span{info, count});
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE size_t cpp_percpu_processor_count() {
  return percpu::processor_count();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE zx_status_t cpp_mp_hotplug_cpu_mask_all() {
  cpu_mask_t all_cpus = ((cpu_mask_t)1u << arch_max_num_cpus()) - 1;
  return mp_hotplug_cpu_mask(~mp_get_online_mask() & all_cpus);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE zx_status_t cpp_mp_unplug_cpu_mask_all_but_primary() {
  cpu_mask_t primary = cpu_num_to_mask(0);
  return mp_unplug_cpu_mask(mp_get_online_mask() & ~primary, ZX_TIME_INFINITE);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_platform_graceful_halt_helper(uint32_t action) {
  platform_graceful_halt_helper(static_cast<platform_halt_action>(action),
                                ZirconCrashReason::NoCrash, ZX_TIME_INFINITE);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE zx_status_t cpp_halt_token_ack_pending_halt() {
  return HaltToken::Get().AckPendingHalt();
}

#if defined(__x86_64__)
// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE zx_status_t
cpp_system_powerctl_x86_set_pkg_pl1(const zx_system_powerctl_arg_t* arg) {
  MsrAccess msr;
  return arch_system_powerctl(ZX_SYSTEM_POWERCTL_X86_SET_PKG_PL1, arg, &msr);
}
#endif

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_wake_vector_discard_wake_event_report() {
  wake_vector::WakeEvent::DiscardWakeEventReport();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE zx_instant_boot_t
cpp_idle_power_thread_transition_all_active_to_suspend(zx_instant_boot_t resume_deadline) {
  return IdlePowerThread::TransitionAllActiveToSuspend(resume_deadline);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE zx_status_t cpp_wake_vector_generate_wake_event_report(
    zx_instant_boot_t start_time, zx_wake_source_report_header_t* out_header,
    zx_wake_source_report_entry_t* out_entries, uint32_t num_entries, uint32_t* actual_entries) {
  return wake_vector::WakeEvent::GenerateWakeEventReport(
      start_time, make_user_out_ptr(out_header), make_user_out_ptr(out_entries), num_entries,
      make_user_out_ptr(actual_entries));
}
