// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "kernel/dpc.h"

#include <assert.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/ops.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/event.h>
#include <kernel/lockdep.h>
#include <kernel/percpu.h>
#include <kernel/scheduler.h>
#include <kernel/spinlock.h>
#include <ktl/bit.h>
#include <lk/init.h>

#define DPC_THREAD_PRIORITY HIGH_PRIORITY

namespace {

// The Dpc thread for timers may use up to 150us out of every 300us (i.e. 50% of the CPU) in
// the worst case. DPCs usually take only a small fraction of this and have a much lower
// frequency than 3.333KHz.
//
// TODO(https://fxbug.dev/42114336): Make this runtime tunable. It may be necessary to change
// the Dpc deadline params later in boot, after configuration is loaded somehow.
constexpr SchedulerState::BaseProfile kProfileLowLatency{
    SchedDeadlineParams{SchedDuration{ZX_USEC(150)}, SchedDuration{ZX_USEC(300)}}};

}  // namespace

void Dpc::Invoke() { func_(this); }

void DpcRunner::InitForCurrentCpu() {
  if constexpr (DEBUG_ASSERT_IMPLEMENTED) {
    Thread* const current_thread = Thread::Current::Get();
    SingleChainLockGuard guard{IrqSaveOption, current_thread->get_lock(),
                               CLT_TAG("DpcRunner::InitForCurrentCpu")};
    const cpu_mask_t mask = current_thread->scheduler_state().hard_affinity();
    DEBUG_ASSERT_MSG(ktl::popcount(mask) == 1, "mask %#x", mask);
  }

  const cpu_num_t cpu = arch_curr_cpu_num();

  {
    Guard<SpinLock, IrqSave> guard{Dpc::Lock::Get()};

    // This cpu's DpcRunner was initialized on a previous hotplug event.
    if (initialized_) {
      return;
    }

    DEBUG_ASSERT(cpu_ == INVALID_CPU);
    cpu_ = cpu;
    initialized_ = true;
  }

  queue_general_.Init(cpu, "dpcg-", SchedulerState::BaseProfile{DEFAULT_PRIORITY});
  queue_low_latency_.Init(cpu, "dpcll-", kProfileLowLatency);
}

zx_status_t DpcRunner::Shutdown(zx_instant_mono_t deadline) {
  zx_status_t status = queue_general_.Shutdown(deadline);
  if (status != ZX_OK) {
    return status;
  }
  return queue_low_latency_.Shutdown(deadline);
}

void DpcRunner::TransitionOffCpu(DpcRunner& source) {
  bool took_from_general = false;
  bool took_from_low_latency = false;

  {
    Guard<SpinLock, IrqSave> guard{Dpc::Lock::Get()};

    // |source|'s cpu is shutting down. Assert that we are migrating to the current cpu.
    DEBUG_ASSERT(cpu_ == arch_curr_cpu_num());
    DEBUG_ASSERT(cpu_ != source.cpu_);

    took_from_general = queue_general_.TakeFromLocked(source.queue_general_);
    took_from_low_latency = queue_low_latency_.TakeFromLocked(source.queue_low_latency_);

    source.initialized_ = false;
    source.cpu_ = INVALID_CPU;
  }

  if (took_from_general) {
    queue_general_.Signal();
  }
  if (took_from_low_latency) {
    queue_low_latency_.Signal();
  }
}

zx_status_t DpcRunner::Enqueue(Dpc& dpc, QueueType type) {
  DpcRunner::Queue* queue = nullptr;
  {
    Guard<SpinLock, IrqSave> guard{Dpc::Lock::Get()};

    if (dpc.InContainer()) {
      return ZX_ERR_ALREADY_EXISTS;
    }

    // Select the queue.
    DpcRunner& runner = percpu::GetCurrent().dpc_runner;
    switch (type) {
      case QueueType::General:
        queue = &runner.queue_general_;
        break;
      case QueueType::LowLatency:
        queue = &runner.queue_low_latency_;
        break;
      default:
        panic("unknown QueueType %u", static_cast<uint32_t>(type));
    };

    // Put this Dpc at the tail of the list. Signal the worker outside the lock.
    queue->EnqueueLocked(dpc);
  }

  // Signal outside of the lock to avoid lock order inversion with the thread
  // lock.
  queue->Signal();
  return ZX_OK;
}

void DpcRunner::Queue::Init(cpu_num_t cpu, const char* name_prefix,
                            const SchedulerState::BaseProfile& profile) {
  char name[ZX_MAX_NAME_LEN];
  snprintf(name, sizeof(name), "%s%u", name_prefix, cpu);

  thread_start_routine entry = [](void* arg) -> int {
    return reinterpret_cast<DpcRunner::Queue*>(arg)->DoWork();
  };

  Thread* thread = Thread::Create(name, entry, this, DPC_THREAD_PRIORITY);
  ASSERT(thread != nullptr);
  thread->SetBaseProfile(profile);
  thread->SetCpuAffinity(cpu_num_to_mask(cpu));

  {
    Guard<SpinLock, IrqSave> guard{Dpc::Lock::Get()};
    thread_ = thread;
  }

  thread->Resume();
}

zx_status_t DpcRunner::Queue::Shutdown(zx_instant_mono_t deadline) {
  Thread* t;
  Event* event;
  {
    Guard<SpinLock, IrqSave> guard{Dpc::Lock::Get()};

    DEBUG_ASSERT(thread_ != nullptr);

    // Ask the thread to terminate.
    DEBUG_ASSERT(!stop_);
    stop_ = true;

    // Remember this Event so we can signal it outside the spinlock.
    event = &event_;

    // Remember the thread so we can join outside the spinlock.
    t = thread_;
    thread_ = nullptr;
  }

  // Wake the thread and wait for it to terminate.
  AutoPreemptDisabler preempt_disabled;
  event->Signal();
  return t->Join(nullptr, deadline);
}

bool DpcRunner::Queue::TakeFromLocked(Queue& source) {
  // The thread must have already been stopped by a call to |Shutdown|.
  DEBUG_ASSERT(source.stop_);
  DEBUG_ASSERT(source.thread_ == nullptr);

  bool moved = !source.list_.is_empty();

  // Move the contents of |source.list_| to the back of our |list_|.
  auto back = list_.end();
  list_.splice(back, source.list_);

  // Reset |source|'s state so we can restart Dpc processing if its cpu comes back online.
  source.event_.Unsignal();
  DEBUG_ASSERT(source.list_.is_empty());
  source.stop_ = false;

  return moved;
}

void DpcRunner::Queue::EnqueueLocked(Dpc& dpc) { list_.push_back(&dpc); }
void DpcRunner::Queue::Signal() { event_.Signal(); }

int DpcRunner::Queue::DoWork() {
  for (;;) {
    // Wait for a Dpc to fire.
    [[maybe_unused]] zx_status_t err = event_.Wait();
    DEBUG_ASSERT(err == ZX_OK);

    ktl::optional<Dpc> dpc_local;
    {
      Guard<SpinLock, IrqSave> guard{Dpc::Lock::Get()};

      if (stop_) {
        return 0;
      }

      // Pop a Dpc off our list, and make a local copy.
      Dpc* dpc = list_.pop_front();

      // If our list is now empty, unsignal the event so we block until it is.
      if (!dpc) {
        event_.Unsignal();
        continue;
      }

      // Copy the Dpc to the stack.
      dpc_local.emplace(*dpc);
    }

    // Call the Dpc.
    dpc_local->Invoke();
  }

  return 0;
}

static void dpc_init(unsigned int level) {
  // Initialize the DpcRunner for the main cpu.
  percpu::GetCurrent().dpc_runner.InitForCurrentCpu();
}

LK_INIT_HOOK(dpc, dpc_init, LK_INIT_LEVEL_THREADING)
