// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <zircon/types.h>

#include <arch/mp_unplug_event.h>
#include <kernel/cpu.h>
#include <kernel/deadline.h>
#include <kernel/ffi.h>
#include <kernel/mp.h>

static_assert(sizeof(mp_ipi_target) == 1, "mp_ipi_target size mismatch");
static_assert(sizeof(mp_ipi) == 1, "mp_ipi size mismatch");
static_assert(static_cast<uint8_t>(mp_ipi_target::MASK) == 0);
static_assert(static_cast<uint8_t>(mp_ipi_target::ALL) == 1);
static_assert(static_cast<uint8_t>(mp_ipi_target::ALL_BUT_LOCAL) == 2);
static_assert(static_cast<uint8_t>(mp_ipi::GENERIC) == 0);
static_assert(static_cast<uint8_t>(mp_ipi::RESCHEDULE) == 1);
static_assert(static_cast<uint8_t>(mp_ipi::INTERRUPT) == 2);
static_assert(static_cast<uint8_t>(mp_ipi::HALT) == 3);

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

void cpp_mp_mbx_generic_irq();
void cpp_mp_mbx_reschedule_irq();
void cpp_mp_mbx_interrupt_irq();

void cpp_mp_sync_exec(mp_ipi_target target, cpu_mask_t mask, mp_sync_task_t task, void* context);

zx_status_t cpp_mp_hotplug_cpu_mask(cpu_mask_t mask);
zx_status_t cpp_mp_hotplug_cpu(cpu_num_t cpu);
void cpp_mp_unplug_current_cpu();
void cpp_mp_unplug_event_signal(MpUnplugEvent* event);
zx_status_t cpp_mp_unplug_cpu_mask(cpu_mask_t mask, zx_instant_mono_t deadline);
zx_status_t cpp_mp_unplug_cpu(cpu_num_t cpu);

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_unplug_event_signal(MpUnplugEvent* event) { event->Signal(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_set_cpu_online(cpu_num_t cpu, bool online) {
  mp_set_cpu_online(cpu, online);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_set_curr_cpu_online(bool online) { mp_set_curr_cpu_online(online); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE cpu_mask_t cpp_mp_get_online_mask() { return mp_get_online_mask(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_mp_is_cpu_online(cpu_num_t cpu) { return mp_is_cpu_online(cpu); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_signal_curr_cpu_ready() { mp_signal_curr_cpu_ready(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_mp_wait_for_all_cpus_ready(const Deadline* deadline) {
  DEBUG_ASSERT(deadline != nullptr);
  return mp_wait_for_all_cpus_ready(*deadline);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_reschedule(cpu_mask_t mask, uint32_t flags) {
  mp_reschedule(mask, flags);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_reschedule_self() { mp_reschedule_self(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_interrupt(mp_ipi_target target, cpu_mask_t mask) {
  mp_interrupt(target, mask);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_mbx_generic_irq() { mp_mbx_generic_irq(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_mbx_reschedule_irq() { mp_mbx_reschedule_irq(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_mbx_interrupt_irq() { mp_mbx_interrupt_irq(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_sync_exec(mp_ipi_target target, cpu_mask_t mask, mp_sync_task_t task,
                                        void* context) {
  mp_sync_exec(target, mask, task, context);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_mp_hotplug_cpu_mask(cpu_mask_t mask) {
  return mp_hotplug_cpu_mask(mask);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_mp_hotplug_cpu(cpu_num_t cpu) { return mp_hotplug_cpu(cpu); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_mp_unplug_current_cpu() { mp_unplug_current_cpu(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_mp_unplug_cpu_mask(cpu_mask_t mask, zx_instant_mono_t deadline) {
  return mp_unplug_cpu_mask(mask, deadline);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_mp_unplug_cpu(cpu_num_t cpu) { return mp_unplug_cpu(cpu); }

}  // extern "C"
