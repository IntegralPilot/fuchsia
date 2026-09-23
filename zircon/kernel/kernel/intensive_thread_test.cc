// Copyright 2016, 2018 The Fuchsia Authors
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <debug.h>
#include <lib/arch/intrin.h>
#include <lib/unittest/unittest.h>
#include <platform.h>
#include <pow2.h>
#include <stdlib.h>
#include <string.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/algorithm.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/event.h>
#include <kernel/mp.h>
#include <kernel/mutex.h>
#include <kernel/scheduler.h>
#include <kernel/thread.h>
#include <ktl/atomic.h>
#include <ktl/iterator.h>

#include <ktl/enforce.h>

// These tests take much longer than, and are more intensive on the scheduler than,
// the //zircon/kernel/kernel/thread.cc tests.

namespace {

static int rand_range(int low, int high) {
  ZX_DEBUG_ASSERT(low <= high);
  uint r = rand();
  return static_cast<int>(((r ^ (r >> 16)) % (high - low + 1u)) + low);
}

static int mutex_thread(void* arg) {
  int i;
  const int iterations = 100000;

  static volatile uintptr_t shared = 0;

  auto m = reinterpret_cast<Mutex*>(arg);

  for (i = 0; i < iterations; i++) {
    m->Acquire();

    if (shared != 0)
      panic("someone else has messed with the shared data\n");

    shared = (intptr_t)Thread::Current::Get();
    if ((rand() % 5) == 0)
      Thread::Current::Yield();

    shared = 0;

    m->Release();
    if ((rand() % 5) == 0)
      Thread::Current::Yield();
  }

  return 0;
}

static Mutex imutex;

bool mutex_test() {
  BEGIN_TEST;

  Mutex m;

  Thread* threads[5];

  for (auto& thread : threads) {
    thread = Thread::Create("mutex tester", &mutex_thread, &m, DEFAULT_PRIORITY);
    thread->Resume();
  }

  for (auto& thread : threads) {
    thread->Join(NULL, ZX_TIME_INFINITE);
  }

  Thread::Current::SleepRelative(ZX_MSEC(100));

  END_TEST;
}

bool mutex_inherit_test() {
  BEGIN_TEST;

  constexpr uint inherit_test_mutex_count = 4;
  constexpr uint inherit_test_thread_count = 5;

  {  // Explicit scope to control when the destruction of |args| happens
    // working variables to pass the working thread
    struct args {
      Event test_blocker;
      Mutex test_mutex[inherit_test_mutex_count];
    } args;

    // worker thread to stress the priority inheritance mechanism
    auto inherit_worker = [](void* arg) TA_NO_THREAD_SAFETY_ANALYSIS -> int {
      struct args* args = static_cast<struct args*>(arg);

      for (int count = 0; count < 10000; count++) {
        uint r = rand_range(1, inherit_test_mutex_count);

        // pick a random priority
        Thread::Current::Get()->SetBaseProfile(
            SchedulerState::BaseProfile{rand_range(DEFAULT_PRIORITY - 4, DEFAULT_PRIORITY + 4)});

        // grab a random number of mutexes
        for (uint j = 0; j < r; j++) {
          args->test_mutex[j].Acquire();
        }

        // wait on an event for a period of time, to try to have other grabber threads
        // need to tweak our priority in either one of the mutexes we hold or the
        // blocking event
        args->test_blocker.WaitDeadline(current_mono_time() + ZX_USEC(rand() % 10u),
                                        Interruptible::Yes);

        // release in reverse order
        for (int j = r - 1; j >= 0; j--) {
          args->test_mutex[j].Release();
        }
      }

      return 0;
    };

    // create a stack of mutexes and a few threads
    Thread* test_thread[inherit_test_thread_count];
    for (auto& t : test_thread) {
      t = Thread::Create("mutex tester", inherit_worker, &args, DEFAULT_PRIORITY);
      t->Resume();
    }

    for (auto& t : test_thread) {
      t->Join(NULL, ZX_TIME_INFINITE);
    }
  }

  Thread::Current::SleepRelative(ZX_MSEC(100));

  END_TEST;
}

static int event_signaler(void* arg) {
  Event* event = static_cast<Event*>(arg);

  // event signaler pausing
  Thread::Current::SleepRelative(ZX_SEC(1));

  event->Signal();
  Thread::Current::Yield();

  return 0;
}

struct WaiterArgs {
  Event* event;
  size_t count;
};

static int event_waiter(void* arg) {
  // Copy our arguments here so we can mutate the count.
  WaiterArgs args = *static_cast<WaiterArgs*>(arg);

  while (args.count > 0) {
    zx_status_t status = args.event->WaitDeadline(ZX_TIME_INFINITE, Interruptible::Yes);
    if (status == ZX_ERR_INTERNAL_INTR_KILLED) {
      return -1;
    } else if (status != ZX_OK) {
      return -1;
    }
    Thread::Current::Yield();
    args.count--;
  }

  return 0;
}

bool event_test() {
  BEGIN_TEST;

  Thread* threads[5];

  {
    /* make sure signaling the event wakes up all the threads and stays signaled */
    Event event;
    WaiterArgs args{&event, 2};
    threads[0] = Thread::Create("event signaler", &event_signaler, &event, DEFAULT_PRIORITY);
    threads[1] = Thread::Create("event waiter 0", &event_waiter, &args, DEFAULT_PRIORITY);
    threads[2] = Thread::Create("event waiter 1", &event_waiter, &args, DEFAULT_PRIORITY);
    threads[3] = Thread::Create("event waiter 2", &event_waiter, &args, DEFAULT_PRIORITY);
    threads[4] = Thread::Create("event waiter 3", &event_waiter, &args, DEFAULT_PRIORITY);

    for (auto& thread : threads)
      thread->Resume();

    for (auto& thread : threads)
      thread->Join(NULL, ZX_TIME_INFINITE);

    Thread::Current::SleepRelative(ZX_SEC(2));
    // destroying event by going out of scope
  }

  {
    AutounsignalEvent event;
    WaiterArgs args{&event, 99};
    /* make sure signaling the event wakes up precisely one thread */
    threads[0] = Thread::Create("event signaler", &event_signaler, &event, DEFAULT_PRIORITY);
    threads[1] = Thread::Create("event waiter 0", &event_waiter, &args, DEFAULT_PRIORITY);
    threads[2] = Thread::Create("event waiter 1", &event_waiter, &args, DEFAULT_PRIORITY);
    threads[3] = Thread::Create("event waiter 2", &event_waiter, &args, DEFAULT_PRIORITY);
    threads[4] = Thread::Create("event waiter 3", &event_waiter, &args, DEFAULT_PRIORITY);

    for (auto& thread : threads)
      thread->Resume();

    Thread::Current::SleepRelative(ZX_SEC(2));

    for (auto& thread : threads) {
      thread->Kill();
      thread->Join(NULL, ZX_TIME_INFINITE);
    }
  }

  END_TEST;
}

static Event context_switch_event;
static Event context_switch_done_event;

static int context_switch_tester(void* arg) {
  int i;
  const int iter = 100000;

  context_switch_event.Wait();

  for (i = 0; i < iter; i++) {
    Thread::Current::Yield();
  }
  Thread::Current::SleepRelative(ZX_SEC(1));

  context_switch_done_event.Signal();

  return 0;
}

bool context_switch_test() {
  BEGIN_TEST;

  Thread::Create("context switch idle", &context_switch_tester, (void*)1, DEFAULT_PRIORITY)
      ->DetachAndResume();
  Thread::Current::SleepRelative(ZX_MSEC(100));
  context_switch_event.Signal();
  context_switch_done_event.Wait();
  Thread::Current::SleepRelative(ZX_MSEC(100));

  context_switch_event.Unsignal();
  context_switch_done_event.Unsignal();
  Thread::Create("context switch 2a", &context_switch_tester, (void*)2, DEFAULT_PRIORITY)
      ->DetachAndResume();
  Thread::Create("context switch 2b", &context_switch_tester, (void*)2, DEFAULT_PRIORITY)
      ->DetachAndResume();
  Thread::Current::SleepRelative(ZX_MSEC(100));
  context_switch_event.Signal();
  context_switch_done_event.Wait();
  Thread::Current::SleepRelative(ZX_MSEC(100));

  context_switch_event.Unsignal();
  context_switch_done_event.Unsignal();
  Thread::Create("context switch 4a", &context_switch_tester, (void*)4, DEFAULT_PRIORITY)
      ->DetachAndResume();
  Thread::Create("context switch 4b", &context_switch_tester, (void*)4, DEFAULT_PRIORITY)
      ->DetachAndResume();
  Thread::Create("context switch 4c", &context_switch_tester, (void*)4, DEFAULT_PRIORITY)
      ->DetachAndResume();
  Thread::Create("context switch 4d", &context_switch_tester, (void*)4, DEFAULT_PRIORITY)
      ->DetachAndResume();
  Thread::Current::SleepRelative(ZX_MSEC(100));
  context_switch_event.Signal();
  context_switch_done_event.Wait();
  Thread::Current::SleepRelative(ZX_MSEC(100));

  END_TEST;
}

static ktl::atomic<int> atomic_var;
static ktl::atomic<int> atomic_count;

static int atomic_tester(void* arg) {
  int add = (int)(uintptr_t)arg;
  int i;

  const int iter = 10000000;

  for (i = 0; i < iter; i++) {
    atomic_var.fetch_add(add);
  }

  atomic_count.fetch_sub(1);

  return 0;
}

bool atomic_test(void) {
  BEGIN_TEST;

  atomic_var = 0;
  atomic_count = 8;

  Thread* threads[8];
  threads[0] = Thread::Create("atomic tester 1", &atomic_tester, (void*)1, LOW_PRIORITY);
  threads[1] = Thread::Create("atomic tester 1", &atomic_tester, (void*)1, LOW_PRIORITY);
  threads[2] = Thread::Create("atomic tester 1", &atomic_tester, (void*)1, LOW_PRIORITY);
  threads[3] = Thread::Create("atomic tester 1", &atomic_tester, (void*)1, LOW_PRIORITY);
  threads[4] = Thread::Create("atomic tester 2", &atomic_tester, (void*)-1, LOW_PRIORITY);
  threads[5] = Thread::Create("atomic tester 2", &atomic_tester, (void*)-1, LOW_PRIORITY);
  threads[6] = Thread::Create("atomic tester 2", &atomic_tester, (void*)-1, LOW_PRIORITY);
  threads[7] = Thread::Create("atomic tester 2", &atomic_tester, (void*)-1, LOW_PRIORITY);

  /* start all the threads */
  for (auto& thread : threads)
    thread->Resume();

  /* wait for them to all stop */
  for (auto& thread : threads) {
    thread->Join(NULL, ZX_TIME_INFINITE);
  }

  END_TEST;
}

static ktl::atomic<int> preempt_count;

static int preempt_tester(void* arg) {
  spin(1000000);

  preempt_count.fetch_sub(1);

  return 0;
}

bool preempt_test() {
  BEGIN_TEST;

  /* create 5 threads, let them run. If the system is properly timer preempting,
   * the threads should interleave each other at a fine enough granularity so
   * that they complete at roughly the same time. */

  preempt_count = 5;

  for (int i = 0; i < preempt_count; i++)
    Thread::Create("preempt tester", &preempt_tester, NULL, LOW_PRIORITY)->DetachAndResume();

  while (preempt_count > 0) {
    Thread::Current::SleepRelative(ZX_SEC(1));
  }

  END_TEST;
}

static int join_tester(void* arg) {
  int val = (int)(uintptr_t)arg;

  Thread::Current::SleepRelative(ZX_MSEC(500));

  return val;
}

static int join_tester_server(void* arg) {
  int ret;
  Thread* t;

  t = Thread::Create("join tester", &join_tester, (void*)1, DEFAULT_PRIORITY);
  t->Resume();
  ret = 99;
  t->canary().Assert();
  ASSERT(ZX_OK == t->Join(&ret, ZX_TIME_INFINITE));

  t = Thread::Create("join tester", &join_tester, (void*)2, DEFAULT_PRIORITY);
  t->Resume();
  Thread::Current::SleepRelative(ZX_SEC(1));  // wait until thread is already dead
  ret = 99;
  t->canary().Assert();
  ASSERT(ZX_OK == t->Join(&ret, ZX_TIME_INFINITE));

  // creating a thread, detaching it, let it exit on its own
  t = Thread::Create("join tester", &join_tester, (void*)3, DEFAULT_PRIORITY);
  t->Detach();
  t->Resume();
  Thread::Current::SleepRelative(ZX_SEC(1));  // wait until the thread should be dead

  // creating a thread, detaching it after it should be dead
  t = Thread::Create("join tester", &join_tester, (void*)4, DEFAULT_PRIORITY);
  t->Resume();
  Thread::Current::SleepRelative(ZX_SEC(1));  // wait until thread is already dead
  t->canary().Assert();
  t->Detach();

  // exiting join tester server

  return 55;
}

bool join_test() {
  BEGIN_TEST;

  int ret;
  Thread* t;

  t = Thread::Create("join tester server", &join_tester_server, (void*)1, DEFAULT_PRIORITY);
  t->Resume();
  ret = 99;
  ASSERT(ZX_OK == t->Join(&ret, ZX_TIME_INFINITE));

  END_TEST;
}

struct lock_pair_t {
  SpinLock first;
  SpinLock second;
};

// Acquires lock on "second" and holds it until it sees that "first" is released.
static int hold_and_release(void* arg) {
  lock_pair_t* pair = reinterpret_cast<lock_pair_t*>(arg);
  ASSERT(pair != nullptr);
  interrupt_saved_state_t state;
  pair->second.AcquireIrqSave(state);
  while (pair->first.HolderCpu() != UINT_MAX) {
    arch::Yield();
  }
  pair->second.ReleaseIrqRestore(state);
  return 0;
}

bool spinlock_test() {
  BEGIN_TEST;

  interrupt_saved_state_t state;
  SpinLock lock;

  // Verify basic functionality (single core).

  // Note that it is invalid the call lock.IsHeld() with interrupts enabled.

  ASSERT(!arch_ints_disabled());
  lock.AcquireIrqSave(state);
  ASSERT(arch_ints_disabled());
  ASSERT(lock.IsHeld());
  ASSERT(lock.HolderCpu() == arch_curr_cpu_num());
  lock.ReleaseIrqRestore(state);
  ASSERT(!arch_ints_disabled());

  // Verify slightly more advanced functionality that requires multiple cores.
  const cpu_mask_t active = Scheduler::PeekActiveMask();
  if (!active || ispow2(active)) {
    printf("skipping rest of spinlock_test, not enough active cpus\n");

    END_TEST;
  }

  lock_pair_t pair;
  Thread* holder_thread =
      Thread::Create("hold_and_release", &hold_and_release, &pair, DEFAULT_PRIORITY);
  ASSERT(holder_thread != nullptr);

  {
    // Disable preemption for the duration the we hold the spinlock to ensure
    // we do not trigger a local reschedule as it would be an error to do so
    // while holding a spinlock.
    AutoPreemptDisabler preempt_disable;

    // Acquire the lock before resuming the thread.
    pair.first.AcquireIrqSave(state);

    // Right now we have suspended IRQs and so we will not be moved off this cpu. To prevent any
    // poor decisions by the scheduler that could cause deadlock we set the affinity of the
    // holder_thread to not include our cpu.
    holder_thread->SetCpuAffinity(active ^ cpu_num_to_mask(arch_curr_cpu_num()));
    holder_thread->Resume();
    while (pair.second.HolderCpu() == UINT_MAX) {
      arch::Yield();
    }

    // See that from our perspective "second" is not held.
    ASSERT(!pair.second.IsHeld());
    pair.first.ReleaseIrqRestore(state);
  }
  holder_thread->Join(NULL, ZX_TIME_INFINITE);

  END_TEST;
}

static int sleeper_kill_thread_infinite_wait(void* arg) {
  Thread::Current::SleepRelative(ZX_MSEC(100));

  printf("sleeper_kill_thread_infinite_wait: waiting until killed\n");
  zx_status_t err = Thread::Current::SleepInterruptible(ZX_TIME_INFINITE);
  ASSERT(err == ZX_ERR_INTERNAL_INTR_KILLED);

  return 0;
}

static int waiter_kill_thread_infinite_wait(void* arg) {
  Event* e = (Event*)arg;

  Thread::Current::SleepRelative(ZX_MSEC(100));

  printf("waiter_kill_thread_infinite_wait: waiting until killed\n");
  zx_status_t err = e->WaitDeadline(ZX_TIME_INFINITE, Interruptible::Yes);
  ASSERT(err == ZX_ERR_INTERNAL_INTR_KILLED);

  return 0;
}

bool kill_test() {
  BEGIN_TEST;

  Thread* t;

  // Starting sleeper thread, then killing it while it sleeps.
  t = Thread::Create("sleeper", sleeper_kill_thread_infinite_wait, 0, LOW_PRIORITY);
  t->Resume();
  Thread::Current::SleepRelative(ZX_MSEC(200));
  t->Kill();
  t->Join(NULL, ZX_TIME_INFINITE);

  // Starting sleeper thread, then killing it before it wakes up.
  t = Thread::Create("sleeper", sleeper_kill_thread_infinite_wait, 0, LOW_PRIORITY);
  t->Resume();
  t->Kill();
  t->Join(NULL, ZX_TIME_INFINITE);

  // Starting sleeper thread, then killing it before it is unsuspended.
  t = Thread::Create("sleeper", sleeper_kill_thread_infinite_wait, 0, LOW_PRIORITY);
  t->Kill();  // kill it before it is resumed
  t->Resume();
  t->Join(NULL, ZX_TIME_INFINITE);

  {
    // Starting waiter thread that waits forever, then killing it while it blocks.
    Event e;
    t = Thread::Create("waiter", waiter_kill_thread_infinite_wait, &e, LOW_PRIORITY);
    t->Resume();
    Thread::Current::SleepRelative(ZX_MSEC(200));
    t->Kill();
    t->Join(NULL, ZX_TIME_INFINITE);
  }

  {
    // Starting waiter thread that waits forever, then killing it before it wakes up.
    Event e;
    t = Thread::Create("waiter", waiter_kill_thread_infinite_wait, &e, LOW_PRIORITY);
    t->Resume();
    t->Kill();
    t->Join(NULL, ZX_TIME_INFINITE);
  }

  END_TEST;
}

struct affinity_test_state {
  Thread* threads[16] = {};
  volatile bool shutdown = false;
};

template <typename T>
static void spin_while(zx_instant_mono_t t, T func) {
  zx_instant_mono_t start = current_mono_time();

  while ((current_mono_time() - start) < t) {
    func();
  }
}

static cpu_mask_t random_mask(cpu_mask_t active) {
  cpu_mask_t r;
  DEBUG_ASSERT(active != 0);
  // Assuming rand is properly random this should converge in 2 iterations on average.
  do {
    r = rand() % active;
  } while (r == 0);
  return r;
}

static int affinity_test_thread(void* arg) {
  affinity_test_state* state = static_cast<affinity_test_state*>(arg);
  const cpu_mask_t active = Scheduler::PeekActiveMask();

  while (!state->shutdown) {
    int which = rand() % static_cast<int>(ktl::size(state->threads));
    switch (rand() % 5) {
      case 0:  // set affinity
        state->threads[which]->SetCpuAffinity((cpu_mask_t)random_mask(active));
        break;
      case 1:  // sleep for a bit
        Thread::Current::SleepRelative(ZX_USEC(rand() % 100));
        break;
      case 2:  // spin for a bit
        spin((uint32_t)rand() % 100);
        break;
      case 3:  // yield
        spin_while(ZX_USEC((uint32_t)rand() % 100), Thread::Current::Yield);
        break;
      case 4:  // reschedule
        spin_while(ZX_USEC((uint32_t)rand() % 100), Thread::Current::Reschedule);
        break;
    }
  }

  return 0;
}

// start a bunch of threads that randomly set the affinity of the other threads
// to random masks while doing various work.
// a successful pass is one where it completes the run without tripping over any asserts
// in the scheduler code.
__NO_INLINE bool affinity_test() {
  BEGIN_TEST;

  const cpu_mask_t active = Scheduler::PeekActiveMask();
  if (!active || ispow2(active)) {
    printf("aborting test, not enough active cpus\n");
    END_TEST;
  }

  affinity_test_state state;

  for (auto& t : state.threads) {
    t = Thread::Create("affinity_tester", &affinity_test_thread, &state, LOW_PRIORITY);
  }

  for (auto& t : state.threads) {
    t->Resume();
  }

  static const int duration = 30;
  for (int i = 0; i < duration; i++) {
    Thread::Current::SleepRelative(ZX_MSEC(250));
  }
  state.shutdown = true;
  Thread::Current::SleepRelative(ZX_SEC(1));

  for (auto& t : state.threads) {
    t->Join(nullptr, ZX_TIME_INFINITE);
  }

  END_TEST;
}

static int prio_test_thread(void* arg) {
  Thread* volatile t = Thread::Current::Get();
  SchedulerState::BaseProfile bp = t->SnapshotBaseProfile();
  ASSERT(bp.discipline == SchedDiscipline::Fair);
  ASSERT(bp.fair.weight == SchedulerState::ConvertPriorityToWeight(LOW_PRIORITY));

  auto ev = (Event*)arg;
  ev->Signal();

  // Busy loop until our priority changes.
  int count = 0;
  for (;;) {
    bp = t->SnapshotBaseProfile();
    ASSERT(bp.discipline == SchedDiscipline::Fair);
    if (bp.fair.weight == SchedulerState::ConvertPriorityToWeight(DEFAULT_PRIORITY)) {
      break;
    }
    ++count;
  }

  ev->Signal();

  // And then when it changes again.
  for (;;) {
    bp = t->SnapshotBaseProfile();
    ASSERT(bp.discipline == SchedDiscipline::Fair);
    if (bp.fair.weight == SchedulerState::ConvertPriorityToWeight(HIGH_PRIORITY)) {
      break;
    }
    ++count;
  }

  return count;
}

__NO_INLINE bool priority_test() {
  BEGIN_TEST;

  Thread* t = Thread::Current::Get();
  SchedulerState::BaseProfile bp = t->SnapshotBaseProfile();

  if (!bp.IsFair() ||
      (bp.fair.weight != SchedulerState::ConvertPriorityToWeight(DEFAULT_PRIORITY))) {
    printf("unexpected initial state, aborting test\n");
    END_TEST;
  }

  t->SetBaseProfile(SchedulerState::BaseProfile{DEFAULT_PRIORITY + 2});
  Thread::Current::SleepRelative(ZX_MSEC(1));
  bp = t->SnapshotBaseProfile();
  ASSERT(bp.IsFair());
  ASSERT(bp.fair.weight == SchedulerState::ConvertPriorityToWeight(DEFAULT_PRIORITY + 2));

  t->SetBaseProfile(SchedulerState::BaseProfile{DEFAULT_PRIORITY - 2});
  Thread::Current::SleepRelative(ZX_MSEC(1));
  bp = t->SnapshotBaseProfile();
  ASSERT(bp.IsFair());
  ASSERT(bp.fair.weight == SchedulerState::ConvertPriorityToWeight(DEFAULT_PRIORITY - 2));

  const cpu_mask_t active = Scheduler::PeekActiveMask();
  if (!active || ispow2(active)) {
    printf("skipping rest, not enough active cpus\n");

    END_TEST;
  }

  AutounsignalEvent ev;

  Thread* nt = Thread::Create("prio-test", prio_test_thread, &ev, LOW_PRIORITY);

  cpu_num_t curr = arch_curr_cpu_num();
  cpu_num_t other;
  if (mp_is_cpu_online(curr + 1)) {
    other = curr + 1;
  } else if (mp_is_cpu_online(curr - 1)) {
    other = curr - 1;
  } else {
    ASSERT(false);
  }

  nt->SetCpuAffinity(cpu_num_to_mask(other));
  nt->Resume();

  ASSERT_OK(ev.WaitDeadline(ZX_TIME_INFINITE, Interruptible::Yes));
  nt->SetBaseProfile(SchedulerState::BaseProfile{DEFAULT_PRIORITY});

  ASSERT_OK(ev.WaitDeadline(ZX_TIME_INFINITE, Interruptible::Yes));
  nt->SetBaseProfile(SchedulerState::BaseProfile{HIGH_PRIORITY});

  int count = 0;
  nt->Join(&count, ZX_TIME_INFINITE);

  END_TEST;
}

struct SpinControl {
  ktl::atomic<bool> done{false};
  uint64_t count{0};
};

int oversubscription_spin_thread(void* arg) {
  SpinControl* control = static_cast<SpinControl*>(arg);
  while (!control->done.load()) {
    control->count++;
  }
  return 0;
}

__NO_INLINE bool critical_oversubscription_test() {
  BEGIN_TEST;

  const cpu_mask_t active_mask = Scheduler::PeekActiveMask();
  if (!active_mask || ispow2(active_mask)) {
    printf("skipping rest, not enough active cpus\n");
    END_TEST;
  }

  const cpu_mask_t current_mask = cpu_num_to_mask(arch_curr_cpu_num());
  const auto restore_affinity =
      fit::defer([previous_affinity = Thread::Current::SetCpuAffinity(current_mask)](void) {
        Thread::Current::SetCpuAffinity(previous_affinity);
      });

  // Pick a target CPU to bind our threads to.
  cpu_num_t target_cpu = highest_cpu_set(active_mask & ~current_mask);
  DEBUG_ASSERT(target_cpu != arch_curr_cpu_num());
  const cpu_mask_t target_mask = cpu_num_to_mask(target_cpu);

  // Both threads demand 100% of the CPU (10ms capacity every 10ms).
  const SchedDeadlineParams params{SchedMs(10), SchedMs(10)};

  SchedulerState::BaseProfile critical_profile{params};
  critical_profile.critical = true;

  SchedulerState::BaseProfile normal_profile{params};
  normal_profile.critical = false;

  SpinControl critical_control;
  SpinControl normal_control;

  Thread* critical_thread = Thread::Create("critical-thread", oversubscription_spin_thread,
                                           &critical_control, DEFAULT_PRIORITY);
  Thread* normal_thread = Thread::Create("normal-thread", oversubscription_spin_thread,
                                         &normal_control, DEFAULT_PRIORITY);

  critical_thread->SetCpuAffinity(target_mask);
  normal_thread->SetCpuAffinity(target_mask);

  critical_thread->SetBaseProfile(critical_profile);
  normal_thread->SetBaseProfile(normal_profile);

  // Start the threads.
  critical_thread->Resume();
  normal_thread->Resume();

  // Let them run for a short duration while oversubscribing the CPU.
  Thread::Current::SleepRelative(ZX_MSEC(250));

  // Signal them to stop.
  critical_control.done.store(true);
  normal_control.done.store(true);

  critical_thread->Join(nullptr, ZX_TIME_INFINITE);
  normal_thread->Join(nullptr, ZX_TIME_INFINITE);

  // The critical thread should have received vastly more CPU time. In a perfect
  // system, normal_counter would be 0, but due to context switching and kernel
  // behavior it might get a few cycles.
  EXPECT_GT(critical_control.count, normal_control.count * 10,
            "Critical thread did not receive expected CPU precedence");

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(intensive_thread_tests)
UNITTEST("affinity_test", affinity_test)
UNITTEST("atomic_test", atomic_test)
UNITTEST("context_switch_test", context_switch_test)
UNITTEST("event_test", event_test)
UNITTEST("join_test", join_test)
UNITTEST("kill_test", kill_test)
UNITTEST("mutex_inherit_test", mutex_inherit_test)
UNITTEST("mutex_test", mutex_test)
UNITTEST("preempt_test", preempt_test)
UNITTEST("priority_test", priority_test)
UNITTEST("spinlock_test", spinlock_test)
UNITTEST("critical_oversubscription_test", critical_oversubscription_test)
UNITTEST_END_TESTCASE(intensive_thread_tests, "intensive_thread", "intensive thread tests")
