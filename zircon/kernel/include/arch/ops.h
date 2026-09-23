// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_ARCH_OPS_H_
#define ZIRCON_KERNEL_INCLUDE_ARCH_OPS_H_

#ifndef __ASSEMBLER__

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/time.h>

#include <kernel/cpu.h>

// Fast routines that most arches will implement inline.
void arch_enable_ints();
void arch_disable_ints();
bool arch_ints_disabled();

cpu_num_t arch_curr_cpu_num();
uint arch_max_num_cpus();
uint arch_cpu_features();

extern "C" uint8_t arch_get_hw_breakpoint_count();
extern "C" uint8_t arch_get_hw_watchpoint_count();

uint32_t arch_address_tagging_features();

// Usually implemented in or called from assembly.
extern "C" {
void arch_clean_cache_range(vaddr_t start, size_t len);
void arch_clean_invalidate_cache_range(vaddr_t start, size_t len);
void arch_invalidate_cache_range(vaddr_t start, size_t len);
void arch_sync_cache_range(vaddr_t start, size_t len);
}  // extern C

// Used to suspend work on a CPU until it is further shutdown.
// This will only be invoked with interrupts disabled.  This function
// must not re-enter the scheduler.
// flush_done should be signaled after state is flushed.
class MpUnplugEvent;
extern "C" void arch_flush_state_and_halt(MpUnplugEvent *flush_done) __NO_RETURN;

// Arch optimized version of a page zero routine against a page aligned buffer.
// Usually implemented in or called from assembly.
extern "C" void arch_zero_page(void *);

// Give the specific arch a chance to override some routines.
#include <arch/arch_ops.h>

// The arch_blocking_disallowed() flag is used to check that in-kernel interrupt
// handlers do not do any blocking operations.  This is a per-CPU flag.
// Various blocking operations, such as mutex.Acquire(), contain assertions
// that arch_blocking_disallowed() is false.
//
// arch_blocking_disallowed() should only be true when interrupts are
// disabled.
inline bool arch_blocking_disallowed() { return READ_PERCPU_FIELD(blocking_disallowed); }

inline void arch_set_blocking_disallowed(bool value) {
  WRITE_PERCPU_FIELD(blocking_disallowed, value);
}

inline uint32_t arch_num_spinlocks_held() { return READ_PERCPU_FIELD(num_spinlocks); }

class IdlePowerThread;  // fwd decl
class ArchIdlePowerThread {
 private:
  friend class IdlePowerThread;

  // ArchIdlePowerThread::EnterIdleState is a method which is closely coupled
  // to the behavior of the IdlePowerThreads (one per CPU).  It should really
  // never be used anywhere else but there (hence the extra effort to bury it as
  // a private member of a trivial container class).
  //
  // When called from IdlePowerThread::Run, the rules are:
  // 1) Preemption *must* be disabled.
  // 2) Interrupts *must* be disabled.
  // 3) Preemption will *not* be re-enabled at any point during the
  //    EnterIdleState method.
  // 4) Interrupts *may* be re-enabled as needed by the architecture specific
  //    implementation, but this is not guaranteed to happen.  If they are
  //    re-enabled, the will be disabled again before the function returns.
  // 5) If an implementation needs to re-enable interrupts during its sequence,
  //    it will *always* do so just before falling into the instruction which
  //    waits for the interrupt intended to kick the system out of idle.  IOW -
  //    if the final entry into the idle-but-waiting-for-irq state requires that
  //    interrupts be re-enabled, the operation can be considered to be "atomic"
  //    from the caller's perspective.  The wakeup interrupt will not be missed
  //    because of a race.
  //
  // For callers, the primary takeaway from these rules is that users *must*
  // disable IRQs, but *must not* rely on interrupts being disabled for the
  // entire time to preserve any of their invariants.
  //
  static void EnterIdleState();
};

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_INCLUDE_ARCH_OPS_H_
