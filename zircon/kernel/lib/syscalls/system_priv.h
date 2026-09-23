// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SYSCALLS_SYSTEM_PRIV_H_
#define ZIRCON_KERNEL_LIB_SYSCALLS_SYSTEM_PRIV_H_

#if defined __x86_64__
#include <arch/x86/platform_access.h>
#endif  //__x86_64__
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/system.h>

#if defined __x86_64__
class MsrAccess;
zx_status_t arch_system_powerctl(uint32_t cmd, const zx_system_powerctl_arg_t* arg, MsrAccess* msr);
#else
zx_status_t arch_system_powerctl(uint32_t cmd, const zx_system_powerctl_arg_t* arg);
#endif

zx_status_t system_mexec_core(zx_handle_t resource, zx_handle_t kernel_vmo,
                              zx_handle_t data_zbi_vmo);

extern "C" {
zx_status_t cpp_system_mexec_payload_get_helper(uint8_t* buffer, size_t buffer_size,
                                                size_t* out_zbi_size);
zx_status_t cpp_system_mexec_core(zx_handle_t resource, zx_handle_t kernel_vmo,
                                  zx_handle_t data_zbi_vmo);
void cpp_scheduler_update_processing_rates(zx_cpu_performance_info_t* info, size_t count);
void cpp_scheduler_update_processing_limits(zx_cpu_perf_limit_t* info, size_t count);
void cpp_scheduler_get_performance_scales(zx_cpu_performance_info_t* info, size_t count);
void cpp_scheduler_get_default_performance_scales(zx_cpu_performance_info_t* info, size_t count);
void cpp_scheduler_get_processing_limits(zx_cpu_perf_limit_t* info, size_t count);
size_t cpp_percpu_processor_count();

zx_status_t cpp_mp_hotplug_cpu_mask_all();
zx_status_t cpp_mp_unplug_cpu_mask_all_but_primary();
void cpp_platform_graceful_halt_helper(uint32_t action);
zx_status_t cpp_halt_token_ack_pending_halt();
#if defined(__x86_64__)
zx_status_t cpp_system_powerctl_x86_set_pkg_pl1(const zx_system_powerctl_arg_t* arg);
#endif
void cpp_wake_vector_discard_wake_event_report();
zx_instant_boot_t cpp_idle_power_thread_transition_all_active_to_suspend(
    zx_instant_boot_t resume_deadline);
zx_status_t cpp_wake_vector_generate_wake_event_report(zx_instant_boot_t start_time,
                                                       zx_wake_source_report_header_t* out_header,
                                                       zx_wake_source_report_entry_t* out_entries,
                                                       uint32_t num_entries,
                                                       uint32_t* actual_entries);
}

#endif  // ZIRCON_KERNEL_LIB_SYSCALLS_SYSTEM_PRIV_H_
