// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

unsafe extern "C" {
    fn cpp_cpu_stats_inc_reschedules();
    fn cpp_cpu_stats_inc_context_switches();
    fn cpp_cpu_stats_inc_preempts();
    fn cpp_cpu_stats_inc_yields();
    fn cpp_cpu_stats_inc_interrupts();
    fn cpp_cpu_stats_inc_timer_ints();
    fn cpp_cpu_stats_inc_timers();
    fn cpp_cpu_stats_inc_perf_ints();
    fn cpp_cpu_stats_inc_syscalls();
    fn cpp_cpu_stats_inc_page_faults();
    fn cpp_cpu_stats_inc_reschedule_ipis();
    fn cpp_cpu_stats_inc_generic_ipis();
}

/// Increment the reschedule counter for the current CPU.
#[inline(always)]
pub fn inc_reschedules() {
    // SAFETY: cpp_cpu_stats_inc_reschedules increments the per-CPU reschedule counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_reschedules() }
}

/// Increment the context switch counter for the current CPU.
#[inline(always)]
pub fn inc_context_switches() {
    // SAFETY: cpp_cpu_stats_inc_context_switches increments the per-CPU context switch counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_context_switches() }
}

/// Increment the preempt counter for the current CPU.
#[inline(always)]
pub fn inc_preempts() {
    // SAFETY: cpp_cpu_stats_inc_preempts increments the per-CPU preempt counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_preempts() }
}

/// Increment the yield counter for the current CPU.
#[inline(always)]
pub fn inc_yields() {
    // SAFETY: cpp_cpu_stats_inc_yields increments the per-CPU yield counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_yields() }
}

/// Increment the interrupt counter for the current CPU.
#[inline(always)]
pub fn inc_interrupts() {
    // SAFETY: cpp_cpu_stats_inc_interrupts increments the per-CPU interrupt counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_interrupts() }
}

/// Increment the timer interrupt counter for the current CPU.
#[inline(always)]
pub fn inc_timer_ints() {
    // SAFETY: cpp_cpu_stats_inc_timer_ints increments the per-CPU timer interrupt counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_timer_ints() }
}

/// Increment the timer callback counter for the current CPU.
#[inline(always)]
pub fn inc_timers() {
    // SAFETY: cpp_cpu_stats_inc_timers increments the per-CPU timer callback counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_timers() }
}

/// Increment the performance monitor interrupt counter for the current CPU.
#[inline(always)]
pub fn inc_perf_ints() {
    // SAFETY: cpp_cpu_stats_inc_perf_ints increments the per-CPU performance interrupt counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_perf_ints() }
}

/// Increment the syscall counter for the current CPU.
#[inline(always)]
pub fn inc_syscalls() {
    // SAFETY: cpp_cpu_stats_inc_syscalls increments the per-CPU syscall counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_syscalls() }
}

/// Increment the page fault counter for the current CPU.
#[inline(always)]
pub fn inc_page_faults() {
    // SAFETY: cpp_cpu_stats_inc_page_faults increments the per-CPU page fault counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_page_faults() }
}

/// Increment the reschedule IPI counter for the current CPU.
#[inline(always)]
pub fn inc_reschedule_ipis() {
    // SAFETY: cpp_cpu_stats_inc_reschedule_ipis increments the per-CPU reschedule IPI counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_reschedule_ipis() }
}

/// Increment the generic IPI counter for the current CPU.
#[inline(always)]
pub fn inc_generic_ipis() {
    // SAFETY: cpp_cpu_stats_inc_generic_ipis increments the per-CPU generic IPI counter
    // for the currently executing CPU and is safe to call from any context.
    unsafe { cpp_cpu_stats_inc_generic_ipis() }
}
