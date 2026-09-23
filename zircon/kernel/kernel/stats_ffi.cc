// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/stats.h>

extern "C" {

void cpp_cpu_stats_inc_reschedules();
void cpp_cpu_stats_inc_context_switches();
void cpp_cpu_stats_inc_preempts();
void cpp_cpu_stats_inc_yields();
void cpp_cpu_stats_inc_interrupts();
void cpp_cpu_stats_inc_timer_ints();
void cpp_cpu_stats_inc_timers();
void cpp_cpu_stats_inc_perf_ints();
void cpp_cpu_stats_inc_syscalls();
void cpp_cpu_stats_inc_page_faults();
void cpp_cpu_stats_inc_reschedule_ipis();
void cpp_cpu_stats_inc_generic_ipis();

void cpp_cpu_stats_inc_reschedules() { CPU_STATS_INC(reschedules); }
void cpp_cpu_stats_inc_context_switches() { CPU_STATS_INC(context_switches); }
void cpp_cpu_stats_inc_preempts() { CPU_STATS_INC(preempts); }
void cpp_cpu_stats_inc_yields() { CPU_STATS_INC(yields); }
void cpp_cpu_stats_inc_interrupts() { CPU_STATS_INC(interrupts); }
void cpp_cpu_stats_inc_timer_ints() { CPU_STATS_INC(timer_ints); }
void cpp_cpu_stats_inc_timers() { CPU_STATS_INC(timers); }
void cpp_cpu_stats_inc_perf_ints() { CPU_STATS_INC(perf_ints); }
void cpp_cpu_stats_inc_syscalls() { CPU_STATS_INC(syscalls); }
void cpp_cpu_stats_inc_page_faults() { CPU_STATS_INC(page_faults); }
void cpp_cpu_stats_inc_reschedule_ipis() { CPU_STATS_INC(reschedule_ipis); }
void cpp_cpu_stats_inc_generic_ipis() { CPU_STATS_INC(generic_ipis); }

}  // extern "C"
