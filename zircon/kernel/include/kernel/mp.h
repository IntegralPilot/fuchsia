// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_MP_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_MP_H_

#include <lib/ktrace.h>
#include <limits.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <fbl/intrusive_double_list.h>
#include <kernel/cpu.h>
#include <kernel/mutex.h>
#include <kernel/spinlock.h>
#include <kernel/thread.h>
#include <ktl/atomic.h>

using mp_ipi_task_func_t = void (*)(void* context);
using mp_sync_task_t = void (*)(void* context);

enum class mp_ipi : uint8_t { GENERIC, RESCHEDULE, INTERRUPT, HALT };

// When sending inter processor interrupts (IPIs), apis will take a combination of
// this enum and a bitmask. If mp_ipi_target::MASK is used, the mask argument will
// contain a bitmap of every cpu that should receive the IPI. The other targets
// serve as shortcuts and potentially optimizations in the lower layers.
enum class mp_ipi_target : uint8_t { MASK, ALL, ALL_BUT_LOCAL };

void mp_init();

// Trigger reschedules on other CPUs. Used mostly by inner threading
// and scheduler logic.
void mp_reschedule(cpu_mask_t mask, uint flags);

// Trigger a reschedule on the calling CPU using an IPI.
void mp_reschedule_self();

// Trigger an interrupt on another cpu without a corresponding reschedule.
// Used by the hypervisor to trigger a vmexit.
void mp_interrupt(mp_ipi_target, cpu_mask_t mask);

// Make a cross cpu call to one or more cpus. Waits for all of the calls
// to complete before returning.
void mp_sync_exec(mp_ipi_target, cpu_mask_t mask, mp_sync_task_t task, void* context);

zx_status_t mp_hotplug_cpu_mask(cpu_mask_t mask);

void mp_unplug_current_cpu();

// Unplug the cpu specified by |mask|, waiting, up to |deadline| for its "shutdown" thread to
// complete.
zx_status_t mp_unplug_cpu_mask(cpu_mask_t mask, zx_instant_mono_t deadline);

inline zx_status_t mp_hotplug_cpu(cpu_num_t cpu) {
  return mp_hotplug_cpu_mask(cpu_num_to_mask(cpu));
}
inline zx_status_t mp_unplug_cpu(cpu_num_t cpu) {
  return mp_unplug_cpu_mask(cpu_num_to_mask(cpu), ZX_TIME_INFINITE);
}

// called from arch code during reschedule irq
void mp_mbx_reschedule_irq();
// called from arch code during generic task irq
void mp_mbx_generic_irq();
// called from arch code during interrupt irq
void mp_mbx_interrupt_irq();

// tracks if a cpu is online and initialized
void mp_set_cpu_online(cpu_num_t cpu, bool online);
inline void mp_set_curr_cpu_online(bool online) { mp_set_cpu_online(arch_curr_cpu_num(), online); }
cpu_mask_t mp_get_online_mask();
bool mp_is_cpu_online(cpu_num_t cpu);

// We say a CPU is "ready" once it is active and generally ready for tasks.
// This function signals that the current CPU is ready, allowing for
// synchronization based on all CPUs reaching this state.
//
// This function is expected to be called once - and only once - within each
// CPU's start-up, after it has been set as active.
void mp_signal_curr_cpu_ready();

// Wait until all of the CPUs in the system are ready (i.e., once all CPUs have
// called `mp_signal_curr_cpu_ready()`).
//
// Note: Do not call this until at least LK_INIT_LEVEL_PLATFORM + 1, or later.
// PLATFORM is the point at which CPUs check in.  If a call it made to wait
// before this, there is a chance that we are on the primary CPU and before the
// point that CPUs have been told to start, or that we are on a secondary CPU
// during early start-up, and we have not reached our check-in point yet.
// Calling this function at a point in a situation like that is a guaranteed
// timeout.
zx_status_t mp_wait_for_all_cpus_ready(Deadline deadline);

extern "C" {
void cpp_mp_set_cpu_online(cpu_num_t cpu, bool online);
void cpp_mp_set_curr_cpu_online(bool online);
cpu_mask_t cpp_mp_get_online_mask();
bool cpp_mp_is_cpu_online(cpu_num_t cpu);
void cpp_mp_signal_curr_cpu_ready();
zx_status_t cpp_mp_wait_for_all_cpus_ready(const Deadline* deadline);
void cpp_mp_reschedule(cpu_mask_t mask, uint32_t flags);
void cpp_mp_reschedule_self();
void cpp_mp_interrupt(mp_ipi_target target, cpu_mask_t mask);
void cpp_mp_sync_exec(mp_ipi_target target, cpu_mask_t mask, mp_sync_task_t task, void* context);
zx_status_t cpp_mp_hotplug_cpu_mask(cpu_mask_t mask);
zx_status_t cpp_mp_hotplug_cpu(cpu_num_t cpu);
void cpp_mp_unplug_current_cpu();
zx_status_t cpp_mp_unplug_cpu_mask(cpu_mask_t mask, zx_instant_mono_t deadline);
zx_status_t cpp_mp_unplug_cpu(cpu_num_t cpu);
}  // extern "C"

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_MP_H_
