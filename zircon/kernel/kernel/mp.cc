// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "kernel/mp.h"

#include <align.h>
#include <assert.h>
#include <debug.h>
#include <lib/arch/intrin.h>
#include <lib/console.h>
#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/lazy_init/lazy_init.h>
#include <lib/lockup_detector.h>
#include <lib/lockup_detector/diagnostics.h>
#include <lib/system-topology.h>
#include <lib/thread_sampler/thread_sampler.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <stdlib.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/mp.h>
#include <arch/ops.h>
#include <dev/interrupt.h>
#include <fbl/algorithm.h>
#include <kernel/cpu.h>
#include <kernel/deadline.h>
#include <kernel/dpc.h>
#include <kernel/event.h>
#include <kernel/mutex.h>
#include <kernel/percpu.h>
#include <kernel/scheduler.h>
#include <kernel/spinlock.h>
#include <kernel/stats.h>
#include <kernel/timer.h>
#include <ktl/bit.h>
#include <lk/init.h>
#include <platform/timer.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

// Counts the number of times a CPU reschedules using a self-IPI.
KCOUNTER(counter_reschedule_self, "mp.reschedule_self")

namespace {

// Represents a pending task for some number of CPUs to execute.
struct mp_ipi_task
    : fbl::DoublyLinkedListable<mp_ipi_task*, fbl::NodeOptions::AllowRemoveFromContainer> {
  mp_ipi_task(mp_ipi_task_func_t func_, void* context_) : func(func_), context(context_) {}

  mp_ipi_task_func_t func;
  void* context;
};

// Global mp state to track what the cpus are up to.
struct mp_state {
  // Cpus that are currently online.
  ktl::atomic<cpu_mask_t> online_cpus;

  SpinLock ipi_task_lock{"mp_state:ipi_task_lock"_intern};
  // The list of outstanding tasks for each CPU to execute. Should only be
  // accessed with the ipi_task_lock held
  fbl::DoublyLinkedList<mp_ipi_task*> ipi_task_list[SMP_MAX_CPUS] TA_GUARDED(ipi_task_lock);

  // lock for serializing CPU hotplug/unplug operations
  DECLARE_MUTEX(mp_state) hotplug_lock;

  // Tracks the CPUs that are "ready".
  // Used below in mp_signal_curr_cpu_ready and mp_wait_for_all_cpus_ready.
  ktl::atomic<cpu_mask_t> ready_cpu_mask{0};

  // Signals when all CPUs are ready.
  Event ready_cpu_event;
};

// The global state structure for the mp subsystem, aligned on cpu cache line to minimize aliasing.
mp_state mp __CPU_ALIGN_EXCLUSIVE;

}  // namespace

// tracks if a cpu is online and initialized
void mp_set_cpu_online(cpu_num_t cpu, bool online) {
  if (online) {
    mp.online_cpus.fetch_or(cpu_num_to_mask(cpu));
  } else {
    mp.online_cpus.fetch_and(~cpu_num_to_mask(cpu));
  }
}

cpu_mask_t mp_get_online_mask() { return mp.online_cpus.load(); }
bool mp_is_cpu_online(cpu_num_t cpu) { return mp_get_online_mask() & cpu_num_to_mask(cpu); }

void mp_init() {}

void mp_reschedule(cpu_mask_t mask, uint flags) {
  DEBUG_ASSERT(arch_ints_disabled());

  const cpu_num_t local_cpu = arch_curr_cpu_num();

  LTRACEF("local %u, mask %#x\n", local_cpu, mask);

  // mask out cpus that do not have an active scheduler, and the local cpu
  mask &= Scheduler::PeekActiveMask();
  mask &= ~cpu_num_to_mask(local_cpu);

  LTRACEF("local %u, post mask target now 0x%x\n", local_cpu, mask);

  // if we have no work to do, return
  if (mask == 0) {
    return;
  }

  arch_mp_reschedule(mask);
}

void mp_reschedule_self() {
  DEBUG_ASSERT(arch_ints_disabled());

  const cpu_num_t local_cpu = arch_curr_cpu_num();
  DEBUG_ASSERT(Scheduler::PeekIsActive(local_cpu));

  LTRACEF("local %u\n", local_cpu);

  kcounter_add(counter_reschedule_self, 1);
  arch_mp_reschedule(cpu_num_to_mask(local_cpu));
}

void mp_interrupt(mp_ipi_target target, cpu_mask_t mask) {
  arch_mp_send_ipi(target, mask, mp_ipi::INTERRUPT);
}

namespace {

struct mp_sync_context {
  mp_sync_task_t task;
  void* task_context;
  // Mask of which CPUs need to finish the task
  ktl::atomic<cpu_mask_t> outstanding_cpus;
};

void mp_sync_task(void* raw_context) {
  auto context = reinterpret_cast<mp_sync_context*>(raw_context);
  context->task(context->task_context);
  // use seq-cst atomic to ensure this update is not seen before the
  // side-effects of context->task
  context->outstanding_cpus.fetch_and(~cpu_num_to_mask(arch_curr_cpu_num()));
}

}  // namespace

/* @brief Execute a task on the specified CPUs, and block on the calling
 *        CPU until all CPUs have finished the task.
 *
 *  If MP_IPI_TARGET_ALL or MP_IPI_TARGET_ALL_BUT_LOCAL is the target, the online CPU
 *  mask will be used to determine actual targets.
 *
 * Interrupts must be disabled if calling with MP_IPI_TARGET_ALL_BUT_LOCAL as target
 *
 * The callback in |task| will always be called with |arch_blocking_disallowed()|
 * set to true and preemption disabled.
 */
void mp_sync_exec(const mp_ipi_target target, cpu_mask_t mask, const mp_sync_task_t task,
                  void* const context) {
  const uint num_cpus = arch_max_num_cpus();

  if (target == mp_ipi_target::ALL) {
    mask = mp_get_online_mask();
  } else if (target == mp_ipi_target::ALL_BUT_LOCAL) {
    // targeting all other CPUs but the current one is hazardous
    // if the local CPU may be changed underneath us
    DEBUG_ASSERT(arch_ints_disabled());
    mask = mp_get_online_mask() & ~cpu_num_to_mask(arch_curr_cpu_num());
  } else {
    // Mask any offline CPUs from target list
    mask &= mp_get_online_mask();
  }

  // disable interrupts so our current CPU doesn't change
  interrupt_saved_state_t irqstate = arch_interrupt_save();
  arch::ThreadMemoryBarrier();

  const cpu_num_t local_cpu = arch_curr_cpu_num();

  // remove self from target lists, since no need to IPI ourselves
  const bool targetting_self = mask & cpu_num_to_mask(local_cpu);
  mask &= ~cpu_num_to_mask(local_cpu);

  // create tasks to enqueue (we need one per target due to each containing
  // a linked list node
  mp_sync_context sync_context = {
      .task = task,
      .task_context = context,
      .outstanding_cpus = mask,
  };

  // Create an array up to SMP_MAX_CPUs of tasks to run, but only initialize
  // the first num_cpus of them to avoid unnecessary work.
  __UNINITIALIZED ktl::array<lazy_init::LazyInit<mp_ipi_task>, SMP_MAX_CPUS> sync_tasks;
  for (cpu_num_t i = 0; i < num_cpus; ++i) {
    sync_tasks[i].Initialize(mp_sync_task, &sync_context);
  }

  // enqueue tasks
  mp.ipi_task_lock.Acquire();
  cpu_mask_t remaining = mask;
  cpu_num_t cpu_id = 0;
  while (remaining && cpu_id < num_cpus) {
    if (remaining & 1) {
      mp.ipi_task_list[cpu_id].push_back(&sync_tasks[cpu_id].Get());
    }
    remaining >>= 1;
    cpu_id++;
  }
  mp.ipi_task_lock.Release();

  // let CPUs know to begin executing
  arch_mp_send_ipi(mp_ipi_target::MASK, mask, mp_ipi::GENERIC);

  if (targetting_self) {
    // Because the task may trigger a local reschedule, we must run it with
    // preemption disabled.
    AutoPreemptDisabler apd;
    bool previous_blocking_disallowed = arch_blocking_disallowed();
    arch_set_blocking_disallowed(true);
    mp_sync_task(&sync_context);
    arch_set_blocking_disallowed(previous_blocking_disallowed);
  }
  arch::ThreadMemoryBarrier();

  // we can take interrupts again once we've executed our task
  arch_interrupt_restore(irqstate);

  bool ints_disabled = arch_ints_disabled();
  // wait for all other CPUs to be done with the context
  while (1) {
    // See comment in mp_unplug_trampoline about related CPU hotplug
    // guarantees.
    cpu_mask_t outstanding = sync_context.outstanding_cpus.load(ktl::memory_order_relaxed);
    cpu_mask_t online = mp_get_online_mask();
    if ((outstanding & online) == 0) {
      break;
    }

    // If interrupts are still disabled, we need to attempt to process any
    // tasks queued for us in order to prevent deadlock.
    if (ints_disabled) {
      // Optimistically check if our task list has work without the lock.
      // mp_mbx_generic_irq will take the lock and check again.
      bool empty = [local_cpu]() TA_NO_THREAD_SAFETY_ANALYSIS {
        return mp.ipi_task_list[local_cpu].is_empty();
      }();
      if (!empty) {
        // Run mp_mbx_generic_irq with preemption disabled to more closely mimic
        // IRQ context.
        AutoPreemptDisabler apd;
        bool previous_blocking_disallowed = arch_blocking_disallowed();
        arch_set_blocking_disallowed(true);
        mp_mbx_generic_irq();
        arch_set_blocking_disallowed(previous_blocking_disallowed);
        continue;
      }
    }

    arch::Yield();
  }
  arch::ThreadMemoryBarrier();

  // make sure the sync_tasks aren't in lists anymore, since they're
  // stack allocated
  mp.ipi_task_lock.AcquireIrqSave(irqstate);
  for (cpu_num_t i = 0; i < num_cpus; ++i) {
    // If a task is still around, it's because the CPU went offline.
    if (sync_tasks[i]->InContainer()) {
      sync_tasks[i]->RemoveFromContainer();
    }
  }
  mp.ipi_task_lock.ReleaseIrqRestore(irqstate);
}

void mp_unplug_current_cpu() {
  // We had better not be holding any OwnedWaitQueues at this point in time
  // (it is unclear how we would have ever obtained any in the first place)
  if constexpr (DEBUG_ASSERT_IMPLEMENTED) {
    Thread* const current_thread = Thread::Current::Get();
    SingleChainLockGuard guard{IrqSaveOption, current_thread->get_lock(),
                               CLT_TAG("mp_unplug_current_cpu")};
    current_thread->wait_queue_state().AssertNoOwnedWaitQueues();
  }

  sampler::sampler_percpu_shutdown();
  lockup_percpu_shutdown();
  Scheduler::MigrateUnpinnedThreads();
  DEBUG_ASSERT(!Scheduler::PeekIsActive(arch_curr_cpu_num()));

  // Now that this CPU is no longer active, it is critical that this thread
  // never block.  If this thread blocks, the scheduler may attempt to select
  // this CPU's power thread to run.  Doing so would violate an invariant: tasks
  // may only be scheduled on active CPUs.
  DEBUG_ASSERT(arch_blocking_disallowed());

  // Note that before this invocation, but after we stopped accepting
  // interrupts, we may have received a synchronous task to perform.
  // Clearing this flag will cause the mp_sync_exec caller to consider
  // this CPU done.  If this CPU comes back online before other all
  // of the other CPUs finish their work (very unlikely, since tasks
  // should be quick), then this CPU may execute the task.
  mp_set_curr_cpu_online(false);

  // Stop and then shutdown this CPU's platform timer.
  platform_stop_timer();
  platform_shutdown_timer();

  // Shutdown the interrupt controller for this CPU.  On some platforms (arm64 with GIC) receiving
  // an interrupt at a powered off CPU can result in implementation defined behavior (including
  // resetting the whole system).
  shutdown_interrupts_curr_cpu();

  // Flush all of our caches and signal offline complete.
  percpu::GetCurrent().idle_power_thread.FlushAndHalt();
}

// Hotplug the given cpus.  Blocks until the CPUs are up, or a failure is
// detected.
//
// This should be called in a thread context
zx_status_t mp_hotplug_cpu_mask(cpu_mask_t cpu_mask) {
  DEBUG_ASSERT(!arch_ints_disabled());
  Guard<Mutex> lock(&mp.hotplug_lock);

  // Make sure all of the requested CPUs are offline
  if (cpu_mask & mp_get_online_mask()) {
    return ZX_ERR_BAD_STATE;
  }

  while (cpu_mask != 0) {
    cpu_num_t cpu_id = highest_cpu_set(cpu_mask);
    cpu_mask &= ~cpu_num_to_mask(cpu_id);

    zx_status_t status = platform_mp_cpu_hotplug(cpu_id);
    if (status != ZX_OK) {
      return status;
    }
  }

  return ZX_OK;
}

namespace {

// Unplug a single CPU.  Must be called while holding the hotplug lock
zx_status_t mp_unplug_cpu_mask_single_locked(cpu_num_t cpu_id, zx_instant_mono_t deadline) {
  percpu& percpu_to_unplug = percpu::Get(cpu_id);

  // Wait for |percpu_to_unplug| to complete any in-progress DPCs and terminate its DPC thread.
  // Later, once nothing is running on it, we'll migrate its queued DPCs to another CPU.
  zx_status_t status = percpu_to_unplug.dpc_runner.Shutdown(deadline);
  if (status != ZX_OK) {
    return status;
  }

  status = platform_mp_prep_cpu_unplug(cpu_id);
  if (status != ZX_OK) {
    return status;
  }

  // Request to take the target offline.
  status = percpu_to_unplug.idle_power_thread.TransitionActiveToOffline(deadline).status;
  if (status != ZX_OK) {
    return status;
  }

  // Now that the cpu is no longer processing tasks, migrate
  // |percpu_to_unplug|'s TimerQueue and DpcRunner to this cpu.
  percpu& current_percpu = percpu::GetCurrent();
  current_percpu.timer_queue.TransitionOffCpu(percpu_to_unplug.timer_queue);
  current_percpu.dpc_runner.TransitionOffCpu(percpu_to_unplug.dpc_runner);

  return platform_mp_cpu_unplug(cpu_id);
}

}  // namespace

// Unplug the given cpus.  Blocks until the CPUs are removed or |deadline| has been reached.
//
// Partial failure may occur (in which some CPUs are removed but not others).
//
// This should be called in a thread context.
//
zx_status_t mp_unplug_cpu_mask(cpu_mask_t cpu_mask, zx_instant_mono_t deadline) {
  DEBUG_ASSERT(!arch_ints_disabled());
  Guard<Mutex> lock(&mp.hotplug_lock);

  // Make sure all of the requested CPUs are online
  if (cpu_mask & ~mp_get_online_mask()) {
    return ZX_ERR_BAD_STATE;
  }

  while (cpu_mask != 0) {
    cpu_num_t cpu_id = highest_cpu_set(cpu_mask);
    cpu_mask &= ~cpu_num_to_mask(cpu_id);

    zx_status_t status = mp_unplug_cpu_mask_single_locked(cpu_id, deadline);
    if (status != ZX_OK) {
      return status;
    }
  }

  return ZX_OK;
}

void mp_mbx_generic_irq() {
  DEBUG_ASSERT(arch_ints_disabled());
  const cpu_num_t local_cpu = arch_curr_cpu_num();

  CPU_STATS_INC(generic_ipis);

  while (1) {
    struct mp_ipi_task* task;
    mp.ipi_task_lock.Acquire();
    task = mp.ipi_task_list[local_cpu].pop_front();
    mp.ipi_task_lock.Release();
    if (task == NULL) {
      break;
    }

    task->func(task->context);
  }
}

void mp_mbx_reschedule_irq() {
  const cpu_num_t cpu = arch_curr_cpu_num();

  LTRACEF("cpu %u\n", cpu);

  CPU_STATS_INC(reschedule_ipis);

  if (Scheduler::PeekIsActive(cpu)) {
    Thread::Current::preemption_state().PreemptSetPending(cpu_num_to_mask(cpu));
  }
}

void mp_mbx_interrupt_irq() {
  const cpu_num_t cpu = arch_curr_cpu_num();

  LTRACEF("cpu %u\n", cpu);

  // do nothing, the entire point of this interrupt is to simply have one
  // delivered to the cpu.
}

zx_status_t platform_mp_cpu_hotplug(cpu_num_t cpu_id) { return arch_mp_cpu_hotplug(cpu_id); }

void mp_signal_curr_cpu_ready() {
  cpu_num_t num = arch_curr_cpu_num();
  DEBUG_ASSERT_MSG(Scheduler::PeekIsActive(num), "CPU %u cannot be ready if it is not yet active",
                   num);
  cpu_mask_t mask = cpu_num_to_mask(num);
  cpu_mask_t ready = mp.ready_cpu_mask.fetch_or(mask) | mask;
  int ready_count = ktl::popcount(ready);
  int max_count = static_cast<int>(arch_max_num_cpus());
  DEBUG_ASSERT(ready_count <= max_count);
  if (ready_count == max_count) {
    mp.ready_cpu_event.Signal();
  }
}

zx_status_t mp_wait_for_all_cpus_ready(Deadline deadline) {
  return mp.ready_cpu_event.Wait(deadline);
}

namespace {

void mp_all_cpu_startup_sync_hook(unsigned int rl) {
  // Before proceeding any further, wait for a _really_ long time to make sure
  // that all of the CPUs are ready.  We really don't want to start user-mode
  // until we have seen all of our CPUs start up.  In addition, there are
  // decisions to be made while setting up the VDSO which can only be made once
  // we have seen all CPUs start up and check-in.  Specifically, on ARM, we may
  // need to install a version of `zx_get_ticks` which is slower, but may be
  // needed to work around certain errata presented in only some revisions of
  // the CPU silicon (something which can only be determined by the core itself
  // as it comes up).
  constexpr zx_duration_mono_t kCpuStartupTimeout = ZX_SEC(30);
  zx_status_t status = mp_wait_for_all_cpus_ready(Deadline::after_mono(kCpuStartupTimeout));
  if (status == ZX_OK) {
    return;
  }

  // Something has gone wrong.  One or more of the secondaries has failed to
  // check-in before the timeout.  We can either try to limp along or fail, hard
  // and fast.
  //
  // On development or engineering builds (LK_DEBUGLEVEL > 0), we will emit an
  // oops and continue booting under the assumption that the system is "under
  // development".  By emitting an oops and continuing, we hope to make it
  // easier for the developer to see that there's a problem.  Separately,
  // automated testing infrastructure is designed to look for and flag oops
  // events.
  //
  // On production builds (LK_DEBUGLEVEL == 0) we're going (attempt to) dump
  // some diagnostic data, and then panic.  This can be counter-intuitive.  The
  // thinking here is that it's better to fail hard and fast than to let the
  // system continue on in an unknown or degraded state.  The recovery mechanism
  // is designed to cope with failures that happen early in boot.  Failures that
  // happen later (think after the netstack is up and running) are less likely
  // to trigger the appropriate recovery response.

  // Build masks containing the CPUs that are online+ready, that are merely
  // online, and that should be online+ready so we can report the ones that are
  // missing.  Note, ready implies online.
  const cpu_mask_t ready_mask = mp.ready_cpu_mask.load(ktl::memory_order_relaxed);
  const cpu_mask_t online_mask = mp_get_online_mask();
  cpu_mask_t expected_ready_mask = 0;
  for (system_topology::Node* node : system_topology::GetSystemTopology().processors()) {
    const zbi_topology_processor_t& processor = node->entity.processor;
    for (int i = 0; i < processor.logical_id_count; i++) {
      const cpu_num_t logical_id = node->entity.processor.logical_ids[i];
      expected_ready_mask |= cpu_num_to_mask(logical_id);
    }
  }

  // Format a message that we can use in both the oops and panic paths.
  char msg[200];
  snprintf(msg, sizeof(msg),
           "At least one CPU has not declared itself to be started after %ld ms "
           "(ready %08x, online %08x, expected %08x)\n\n",
           kCpuStartupTimeout / ZX_MSEC(1), ready_mask, online_mask, expected_ready_mask);

  // Is this a development build?
  if (LK_DEBUGLEVEL > 0) {
    KERNEL_OOPS("%s", msg);
    return;
  }

  // This is a production build.  Try to gather some diagnostic data from the
  // CPUs that failed to check-in.
  //
  // Start the panic process so that anything we print from here on out will go
  // out to serial.  Also, be sure to not attempt to halt any other CPUs since
  // we're about to query them for their state.
  platform_panic_start(PanicStartHaltOtherCpus::No);
  printf("%s", msg);

  // If this machine has the ability to dump diagnostic state do so for each CPU
  // that failed to check-in.
  if (CanDumpRegistersAndBacktrace()) {
    cpu_mask_t dump_mask = expected_ready_mask & ~ready_mask;
    cpu_num_t target_cpu;
    while ((target_cpu = remove_cpu_from_mask(dump_mask)) != INVALID_CPU) {
      status = DumpRegistersAndBacktrace(target_cpu, stdout);
      if (status != ZX_OK) {
        printf("failed to dump state for cpu-%u, status %d\n", target_cpu, status);
      }
    }
  }

  platform_halt(HALT_ACTION_HALT, ZirconCrashReason::Panic);
}

// Before allowing the system to proceed to the USER init level, wait to be sure
// that all of the CPUs have started and made it to the check-in point (see
// above).
LK_INIT_HOOK(mp_all_cpu_startup_sync, mp_all_cpu_startup_sync_hook, LK_INIT_LEVEL_SMP_WAIT)

int cmd_mp(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc < 2) {
    printf("not enough arguments\n");
  usage:
    printf("usage:\n");
    printf("%s unplug <cpu_id>\n", argv[0].str);
    printf("%s hotplug <cpu_id>\n", argv[0].str);
    printf("%s reschedule <cpu_id>        : send a reschedule ipi to <cpu_id>\n", argv[0].str);
    return ZX_ERR_INTERNAL;
  }

  if (!strcmp(argv[1].str, "unplug")) {
    if (argc < 3) {
      printf("specify a cpu_id\n");
      goto usage;
    }
    zx_status_t status = mp_unplug_cpu(static_cast<cpu_num_t>(argv[2].u));
    printf("CPU %lu unplug %s %d\n", argv[2].u, (status == ZX_OK ? "succeeded" : "failed"), status);
  } else if (!strcmp(argv[1].str, "hotplug")) {
    if (argc < 3) {
      printf("specify a cpu_id\n");
      goto usage;
    }
    zx_status_t status = mp_hotplug_cpu(static_cast<cpu_num_t>(argv[2].u));
    printf("CPU %lu hotplug %s %d\n", argv[2].u, (status == ZX_OK ? "succeeded" : "failed"),
           status);
  } else if (!strcmp(argv[1].str, "reschedule")) {
    if (argc < 3) {
      printf("specify a cpu_id\n");
      goto usage;
    }

    auto target_cpu = static_cast<cpu_num_t>(argv[2].u);
    if (!Scheduler::PeekIsActive(target_cpu)) {
      printf("target cpu %u is not active\n", target_cpu);
      return ZX_OK;
    }

    cpu_mask_t mask = cpu_num_to_mask(target_cpu);
    cpu_num_t sending_cpu;
    {
      // Disable interrupts so that the sending CPU we record here cannot change
      // during our call to mp_reschedule.
      InterruptDisableGuard irqd;
      sending_cpu = arch_curr_cpu_num();
      mp_reschedule(mask, 0);
    }

    if (sending_cpu == target_cpu) {
      printf("sending cpu is same as target cpu, no ipi sent\n");
    } else {
      printf("sent reschedule ipi to cpu %u\n", target_cpu);
    }
  } else {
    printf("unknown command\n");
    goto usage;
  }

  return ZX_OK;
}

STATIC_COMMAND_START
STATIC_COMMAND("mp", "mp test commands", &cmd_mp)
STATIC_COMMAND_END(mp)

}  // namespace
