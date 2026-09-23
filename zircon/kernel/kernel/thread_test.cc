// Copyright 2016, 2018 The Fuchsia Authors
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <debug.h>
#include <lib/arch/intrin.h>
#include <lib/backtrace.h>
#include <lib/fit/defer.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/unittest/unittest.h>
#include <lib/zircon-internal/macros.h>
#include <pow2.h>
#include <stdlib.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <fbl/array.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/cpu.h>
#include <kernel/event.h>
#include <kernel/mp.h>
#include <kernel/mutex.h>
#include <kernel/percpu.h>
#include <kernel/scheduler.h>
#include <kernel/task_runtime_stats.h>
#include <kernel/thread.h>
#include <ktl/array.h>
#include <ktl/atomic.h>
#include <ktl/bit.h>
#include <ktl/unique_ptr.h>

#include <ktl/enforce.h>

namespace {

// Wait for condition |cond| to become true, polling with slow exponential backoff
// to avoid pegging the CPU.
template <typename F>
void wait_for_cond(F cond) {
  zx_duration_mono_t wait_duration = ZX_USEC(1);
  while (!cond()) {
    Thread::Current::SleepRelative(wait_duration);
    // Increase wait_duration time by ~10%.
    wait_duration += (wait_duration / 10u + 1u);
  }
}

// Create and manage a spinning thread.
class WorkerThread {
 public:
  explicit WorkerThread(const char* name) {
    thread_ = Thread::Create(name, &WorkerThread::WorkerBody, this, LOW_PRIORITY);
    ASSERT(thread_ != nullptr);
  }

  ~WorkerThread() { Join(); }

  void Start() { thread_->Resume(); }

  void Join() {
    if (thread_ != nullptr) {
      worker_should_stop_.store(1);
      int unused_retcode;
      zx_status_t result = thread_->Join(&unused_retcode, ZX_TIME_INFINITE);
      ASSERT(result == ZX_OK);
      thread_ = nullptr;
    }
  }

  void WaitForWorkerProgress() {
    int start_iterations = worker_iterations();
    wait_for_cond([start_iterations, this]() { return start_iterations != worker_iterations(); });
  }

  Thread* thread() const { return thread_; }

  int worker_iterations() { return worker_iterations_.load(); }

  DISALLOW_COPY_ASSIGN_AND_MOVE(WorkerThread);

 private:
  static int WorkerBody(void* arg) {
    auto* self = reinterpret_cast<WorkerThread*>(arg);
    while (self->worker_should_stop_.load() == 0) {
      self->worker_iterations_.fetch_add(1);
    }
    return 0;
  }

  ktl::atomic<int> worker_iterations_{0};
  ktl::atomic<int> worker_should_stop_{0};
  Thread* thread_;
};

bool set_affinity_self_test() {
  BEGIN_TEST;

  // Our worker thread will attempt to schedule itself onto each core, one at
  // a time, and ensure it landed in the right location.
  cpu_mask_t online_cpus = mp_get_online_mask();
  ASSERT_NE(online_cpus, 0u, "Expected at least one CPU to be online.");
  auto worker_body = +[](void* arg) -> int {
    cpu_mask_t& online_cpus = *reinterpret_cast<cpu_mask_t*>(arg);
    Thread* const self = Thread::Current::Get();

    for (cpu_num_t c = 0u; c <= highest_cpu_set(online_cpus); c++) {
      // Skip offline CPUs.
      if ((cpu_num_to_mask(c) & online_cpus) == 0) {
        continue;
      }

      // Set affinity to the given core.
      self->SetCpuAffinity(cpu_num_to_mask(c));

      // Ensure we are on the correct CPU.
      const cpu_num_t current_cpu = arch_curr_cpu_num();
      if (current_cpu != c) {
        UNITTEST_FAIL_TRACEF("Expected to be running on CPU %u, but actually running on %u.", c,
                             current_cpu);
        return ZX_ERR_INTERNAL;
      }
    }

    return ZX_OK;
  };
  Thread* worker =
      Thread::Create("set_affinity_self_test_worker", worker_body, &online_cpus, DEFAULT_PRIORITY);
  ASSERT_NONNULL(worker, "thread_create failed.");
  worker->Resume();

  // Wait for the worker thread to test itself.
  int worker_retcode;
  ASSERT_EQ(worker->Join(&worker_retcode, ZX_TIME_INFINITE), ZX_OK, "Failed to join thread.");
  EXPECT_EQ(worker_retcode, ZX_OK, "Worker thread failed.");

  END_TEST;
}

bool set_affinity_other_test() {
  BEGIN_TEST;

  struct WorkerState {
    ktl::atomic<cpu_num_t> current_cpu{INVALID_CPU};
    ktl::atomic<int> should_stop{0};
  } state;

  // Start a worker, which reports the CPU it is running on.
  auto worker_body = [](void* arg) -> int {
    WorkerState& state = *reinterpret_cast<WorkerState*>(arg);
    while (state.should_stop.load() != 1) {
      state.current_cpu.store(arch_curr_cpu_num());
    }
    return 0;
  };
  Thread* worker =
      Thread::Create("set_affinity_other_test_worker", worker_body, &state, LOW_PRIORITY);
  worker->Resume();

  // Migrate the worker task amongst different threads.
  const cpu_mask_t online_cpus = mp_get_online_mask();
  ASSERT_NE(online_cpus, 0u, "Expected at least one CPU to be online.");
  for (cpu_num_t c = 0u; c <= highest_cpu_set(online_cpus); c++) {
    // Skip offline CPUs.
    if ((cpu_num_to_mask(c) & online_cpus) == 0) {
      continue;
    }

    // Set affinity to the given core.
    worker->SetCpuAffinity(cpu_num_to_mask(c));

    // Wait for it to land on the correct CPU.
    wait_for_cond([c, &state]() { return state.current_cpu.load() == c; });
  }

  // Done.
  state.should_stop.store(1);
  int worker_retcode;
  ASSERT_EQ(worker->Join(&worker_retcode, ZX_TIME_INFINITE), ZX_OK, "Failed to join thread.");

  END_TEST;
}

bool thread_last_cpu_new_thread() {
  BEGIN_TEST;

  // Create a worker, but don't start it.
  WorkerThread worker("last_cpu_new_thread");

  // Ensure we get INVALID_CPU as last cpu.
  ASSERT_EQ(worker.thread()->LastCpu(), INVALID_CPU, "Last CPU on unstarted thread invalid.");

  // Clean up the thread.
  worker.Start();
  worker.Join();

  END_TEST;
}

bool thread_last_cpu_running_thread() {
  BEGIN_TEST;

  WorkerThread worker("last_cpu_running_thread");
  worker.Start();

  // Migrate the worker task across different CPUs.
  const cpu_mask_t online_cpus = mp_get_online_mask();
  ASSERT_NE(online_cpus, 0u, "Expected at least one CPU to be online.");
  for (cpu_num_t c = 0u; c <= highest_cpu_set(online_cpus); c++) {
    // Skip offline CPUs.
    if ((cpu_num_to_mask(c) & online_cpus) == 0) {
      continue;
    }

    // Set affinity to the given core.
    worker.thread()->SetCpuAffinity(cpu_num_to_mask(c));

    // Ensure it is reported at the correct CPU.
    wait_for_cond([c, &worker]() { return worker.thread()->LastCpu() == c; });
  }

  END_TEST;
}

bool thread_empty_soft_affinity_mask() {
  BEGIN_TEST;

  WorkerThread worker("empty_soft_affinity_mask");
  worker.Start();

  // Wait for the thread to start running.
  worker.WaitForWorkerProgress();

  // Set affinity to an invalid (empty) mask.
  worker.thread()->SetSoftCpuAffinity(0);

  // Ensure that the thread is still running.
  worker.WaitForWorkerProgress();

  END_TEST;
}

bool thread_conflicting_soft_and_hard_affinity() {
  BEGIN_TEST;

  // Find two different CPUs to run our tests on.
  cpu_mask_t online = mp_get_online_mask();
  ASSERT_TRUE(online != 0, "No CPUs online.");
  cpu_num_t a = highest_cpu_set(online);
  cpu_num_t b = lowest_cpu_set(online);
  if (a == b) {
    // Skip test on single CPU machines.
    unittest_printf("Only 1 CPU active in this machine. Skipping test.\n");
    return true;
  }

  WorkerThread worker("conflicting_soft_and_hard_affinity");
  worker.Start();

  // Set soft affinity to CPU A, wait for the thread to start running there.
  worker.thread()->SetSoftCpuAffinity(cpu_num_to_mask(a));
  wait_for_cond([&worker, a]() { return worker.thread()->LastCpu() == a; });

  // Set hard affinity to CPU B, ensure the thread migrates there.
  worker.thread()->SetCpuAffinity(cpu_num_to_mask(b));
  wait_for_cond([&worker, b]() { return worker.thread()->LastCpu() == b; });

  // Remove the hard affinity. Make sure the thread migrates back to CPU A.
  worker.thread()->SetCpuAffinity(CPU_MASK_ALL);
  wait_for_cond([&worker, a]() { return worker.thread()->LastCpu() == a; });

  END_TEST;
}

bool set_migrate_fn_test() {
  BEGIN_TEST;

  cpu_mask_t active_cpus = Scheduler::PeekActiveMask();
  if (active_cpus == 0 || ispow2(active_cpus)) {
    printf("Expected multiple CPUs to be active.\n");
    return true;
  }

  // The worker thread will attempt to migrate to another CPU.
  auto worker_body = [](void* arg) -> int {
    cpu_num_t current_cpu = arch_curr_cpu_num();
    Thread* self = Thread::Current::Get();
    self->SetCpuAffinity(Scheduler::PeekActiveMask() ^ cpu_num_to_mask(current_cpu));

    cpu_num_t target_cpu = arch_curr_cpu_num();
    if (current_cpu == target_cpu) {
      UNITTEST_FAIL_TRACEF("Expected to be running on CPU %u, but actually running on %u.",
                           target_cpu, current_cpu);
      return ZX_ERR_INTERNAL;
    }

    return ZX_OK;
  };
  Thread* worker =
      Thread::Create("set_migrate_fn_test_worker", worker_body, nullptr, DEFAULT_PRIORITY);
  ASSERT_NONNULL(worker, "thread_create failed.");

  // Set the migrate function, and begin execution.
  struct {
    int count = 0;
    cpu_num_t last_cpu = INVALID_CPU;
    Thread::MigrateStage next_stage = Thread::MigrateStage::Save;
    bool success = true;
  } migrate_state;

  worker->SetMigrateFn([&migrate_state](Thread* thread, Thread::MigrateStage stage) {
    ChainLockTransaction::AssertActive();
    thread->get_lock().AssertHeld();
    ++migrate_state.count;

    cpu_num_t current_cpu = arch_curr_cpu_num();
    if ((stage == Thread::MigrateStage::Restore) && (migrate_state.last_cpu == current_cpu)) {
      UNITTEST_FAIL_TRACEF("Expected to have migrated CPU.");
      migrate_state.success = false;
    }
    migrate_state.last_cpu = current_cpu;

    if (migrate_state.next_stage != stage) {
      UNITTEST_FAIL_TRACEF("Expected migrate stage %d, but received migrate stage %d.",
                           static_cast<int>(migrate_state.next_stage), static_cast<int>(stage));
      migrate_state.success = false;
    }

    switch (migrate_state.next_stage) {
      case Thread::MigrateStage::Save:
        migrate_state.next_stage = Thread::MigrateStage::Restore;
        break;
      case Thread::MigrateStage::Restore:
        migrate_state.next_stage = Thread::MigrateStage::Exiting;
        if (thread->LastCpuLocked() != current_cpu) {
          UNITTEST_FAIL_TRACEF("Expected last CPU to be current CPU after migrate.");
          migrate_state.success = false;
        }
        break;
      case Thread::MigrateStage::Exiting:
        break;
    }
  });
  worker->Resume();

  // Wait for the worker thread to test itself.
  int worker_retcode;
  ASSERT_EQ(worker->Join(&worker_retcode, ZX_TIME_INFINITE), ZX_OK, "Failed to join thread.");
  EXPECT_EQ(worker_retcode, ZX_OK, "Worker thread failed.");
  EXPECT_EQ(migrate_state.count, 3, "Migrate function was not called 3 times.");
  EXPECT_TRUE(migrate_state.success, "Migrate function was not called with the expected state.")

  END_TEST;
}

bool set_migrate_ready_threads_test() {
  BEGIN_TEST;

  cpu_mask_t active_cpus = Scheduler::PeekActiveMask();
  if (active_cpus == 0 || ispow2(active_cpus)) {
    printf("Expected multiple CPUs to be active.\n");
    return true;
  }

  const cpu_num_t kStartingCpu = 0;
  const cpu_num_t kTargetCpu = 1;

  // The worker thread will validate that it is running on the target CPU.
  const thread_start_routine worker_body = [](void* arg) -> int {
    const cpu_num_t current_cpu = arch_curr_cpu_num();
    if (current_cpu != kTargetCpu) {
      UNITTEST_FAIL_TRACEF("Expected to be running on CPU %u, but actually running on %u.",
                           kTargetCpu, current_cpu);
      return ZX_ERR_INTERNAL;
    }
    return ZX_OK;
  };

  ktl::array<Thread*, 4> workers{nullptr, nullptr, nullptr, nullptr};

  for (auto& worker : workers) {
    worker = Thread::Create("set_migrate_ready_threads_test_worker", worker_body, nullptr,
                            DEFAULT_PRIORITY);
    ASSERT_NONNULL(worker, "thread_create failed.");
    worker->SetCpuAffinity(cpu_num_to_mask(kStartingCpu));
  }

  // Move the test thread to the same CPU that the workers will start on.
  Thread* const current_thread = Thread::Current::Get();
  cpu_mask_t original_affinity = current_thread->GetCpuAffinity();
  current_thread->SetCpuAffinity(cpu_num_to_mask(kStartingCpu));
  ASSERT_EQ(arch_curr_cpu_num(), kStartingCpu, "Failed to move test thread to the starting CPU.");

  auto cleanup = fit::defer([current_thread, original_affinity]() {
    // Restore original CPU affinity of the test thread.
    current_thread->SetCpuAffinity(original_affinity);
  });

  {
    AutoPreemptDisabler preempt_disabled_guard;
    const auto context_switches_before = percpu::GetCurrent().stats.context_switches;

    // Resume the workers with preemption disabled. The workers should stack up
    // behind the current thread in the run queue. BE CAREFUL not to do anything
    // that would block until the workers are validated.
    for (Thread* worker : workers) {
      worker->Resume();

      // Validate the thread state.
      thread_state state;
      cpu_num_t curr_cpu;
      {
        SingleChainLockGuard guard{IrqSaveOption, worker->get_lock(),
                                   CLT_TAG("set_migrate_ready_threads_test")};
        state = worker->state();
        curr_cpu = worker->scheduler_state().curr_cpu();
      }
      ASSERT_EQ(state, THREAD_READY, "The worker was in the wrong state.");
      ASSERT_EQ(curr_cpu, kStartingCpu, "The worker was assigned to the wrong CPU.");
    }

    // Migrate the ready threads to a different CPU. BE CAREFUL not to do
    // anything that would block until the workers are migrated.
    for (Thread* worker : workers) {
      worker->SetCpuAffinity(cpu_num_to_mask(kTargetCpu));
    }

    const auto context_switches_after = percpu::GetCurrent().stats.context_switches;
    ASSERT_EQ(context_switches_before, context_switches_after,
              "The test thread context switched during the critical section.");
  }

  // Wait for the worker thread results.
  for (Thread* worker : workers) {
    int worker_retcode;
    ASSERT_EQ(worker->Join(&worker_retcode, ZX_TIME_INFINITE), ZX_OK, "Failed to join thread.");
    EXPECT_EQ(worker_retcode, ZX_OK, "Worker thread failed.");
  }

  END_TEST;
}

bool migrate_unpinned_threads_test() {
  BEGIN_TEST;
  const cpu_mask_t active_cpus = Scheduler::PeekActiveMask();
  if (active_cpus == 0 || ispow2(active_cpus)) {
    printf("Expected multiple CPUs to be active.\n");
    return true;
  }

  const cpu_num_t kStartingCpu = 1;
  enum class TestState {
    WaitingForWorkerStart,
    WaitingForMigration,
    MigrationDone,
  };
  ktl::atomic<TestState> test_state{TestState::WaitingForWorkerStart};

  // Setup the thread that will be migrated.
  auto worker_body = [](void* arg) -> int {
    ktl::atomic<TestState>& test_state = *(static_cast<ktl::atomic<TestState>*>(arg));

    DEBUG_ASSERT(test_state.load() == TestState::WaitingForWorkerStart);
    test_state.store(TestState::WaitingForMigration);
    while (test_state.load() != TestState::MigrationDone) {
      Thread::Current::SleepRelative(ZX_USEC(100));
    }
    return 0;
  };

  auto fn = [&test_state](Thread* thread, Thread::MigrateStage stage) {
    test_state.store(TestState::MigrationDone);
  };
  Thread* worker = Thread::Create("worker", worker_body, &test_state, DEFAULT_PRIORITY);
  worker->SetSoftCpuAffinity(cpu_num_to_mask(kStartingCpu));
  worker->SetMigrateFn(fn);
  worker->Resume();

  while (test_state.load() != TestState::WaitingForMigration) {
    Thread::Current::SleepRelative(ZX_USEC(100));
  }

  // Setup the thread that will perform the migration.  Note that this is a
  // trampoline function implementation.  It must call
  // Scheduler::TrampolineLockHandoff to deal with dropping locks held across
  // context switches while maintaining ChainLockTransaction invariants.
  //
  auto migrate_body = []() __NO_RETURN {
    Scheduler::TrampolineLockHandoff();
    Scheduler::MigrateUnpinnedThreads();
    Scheduler::SetCurrCpuActive(true);
    arch_enable_ints();
    Thread::Current::Exit(0);
  };
  Thread* migrate = Thread::CreateEtc(nullptr, "migrate", nullptr, nullptr,
                                      SchedulerState::BaseProfile{DEFAULT_PRIORITY}, migrate_body);
  migrate->SetCpuAffinity(cpu_num_to_mask(kStartingCpu));
  migrate->Resume();

  int retcode;
  ASSERT_EQ(migrate->Join(&retcode, ZX_TIME_INFINITE), ZX_OK, "Failed to join migrate thread.");
  EXPECT_EQ(retcode, ZX_OK, "Migrate thread failed.");
  ASSERT_EQ(worker->Join(&retcode, ZX_TIME_INFINITE), ZX_OK, "Failed to join worker thread.");
  EXPECT_EQ(retcode, ZX_OK, "Worker thread failed.");

  END_TEST;
}

bool migrate_stress_test() {
  BEGIN_TEST;

  if (true) {
    // TODO(https://fxbug.dev/78695): Disabled until root cause of hangs on some hardware can be
    // determined.
    printf("Test disabled due to https://fxbug.dev/42158849\n");
    END_TEST;
  }

  // Get number of CPUs in the system.
  const int active_cpus = ktl::popcount(Scheduler::PeekActiveMask());
  printf("Found %d active CPU(s)\n", active_cpus);
  if (active_cpus <= 1) {
    printf("Test can only proceed with multiple active CPUs.\n");
    return true;
  }

  // The worker thread body.
  //
  // Each thread spins ensuring that it is only running on a single, particular
  // CPU. The migration function below updates which CPU the thread is allowed
  // to run on.
  struct ThreadState {
    Thread* thread;
    ktl::atomic<bool> should_stop = false;
    ktl::atomic<bool> started = false;

    // Prevent reporting lock violations inside the migration function to avoid reentering the
    // scheduler and interfering with the migration function state.
    // TODO(https://fxbug.dev/42157332): Figure out how to support violation reporting in sensitive
    // code paths.
    DECLARE_SPINLOCK(ThreadState, lockdep::LockFlagsReportingDisabled) lock;

    cpu_num_t allowed_cpu TA_GUARDED(lock) = 0;
    bool is_migrating TA_GUARDED(lock) = false;
  };

  auto worker_body = +[](void* arg) -> int {
    ThreadState* state = static_cast<ThreadState*>(arg);
    state->started = true;

    while (!state->should_stop.load(ktl::memory_order_relaxed)) {
      {
        Guard<SpinLock, IrqSave> guard(&state->lock);
        ZX_ASSERT_MSG(!state->is_migrating,
                      "Worker thread scheduled between MigrationStage::Before and "
                      "MigrationStage::After being called.\n");
        ZX_ASSERT_MSG(arch_curr_cpu_num() == state->allowed_cpu,
                      "Worker thread running on unexpected CPU: expected = %u, running on = %u\n",
                      state->allowed_cpu, arch_curr_cpu_num());
      }
      Thread::Current::Yield();
    }
    return 0;
  };

  // CPU that all threads start running on. Can be any valid CPU.
  const cpu_num_t starting_cpu = arch_curr_cpu_num();

  // Create threads.
  const int num_threads = active_cpus;
  fbl::AllocChecker ac;
  fbl::Array<ThreadState> threads(new (&ac) ThreadState[num_threads], num_threads);
  ZX_ASSERT(ac.check());
  for (int i = 0; i < num_threads; i++) {
    // Create the thread.
    threads[i].thread =
        Thread::Create("migrate_stress_test worker", worker_body, &threads[i], DEFAULT_PRIORITY);
    ASSERT_NONNULL(threads[i].thread, "Thread::Create failed.");
    {
      Guard<SpinLock, IrqSave> guard(&threads[i].lock);
      threads[i].allowed_cpu = starting_cpu;
    }

    auto migrate_fn = [&state = threads[i]](Thread* thread, Thread::MigrateStage stage) {
      Guard<SpinLock, IrqSave> guard(&state.lock);
      switch (stage) {
        case Thread::MigrateStage::Save:
          ZX_ASSERT_MSG(!state.is_migrating, "Thread is already migrating");
          ZX_ASSERT_MSG(state.allowed_cpu == arch_curr_cpu_num(),
                        "Migrate function called on incorrect CPU.");
          state.allowed_cpu = -1;
          state.is_migrating = true;
          break;

        case Thread::MigrateStage::Restore:
          ZX_ASSERT(state.is_migrating);
          state.is_migrating = false;
          state.allowed_cpu = arch_curr_cpu_num();
          break;

        case Thread::MigrateStage::Exiting:
          break;
      }
    };

    // Set the migration function to ensure that `allowed_cpu` keeps up to date.
    threads[i].thread->SetMigrateFn(migrate_fn);

    // Start running on `starting_cpu`.
    threads[i].thread->SetSoftCpuAffinity(1 << starting_cpu);
    threads[i].thread->Resume();
  }

  // Wait for the worker threads to execute at least once to ensure the migrate function gets
  // called. Threads that have never executed are moved without calling the migrate function, which
  // could cause this test to incorrectly assert in the worker loop.
  for (const auto& thread_state : threads) {
    while (!thread_state.started) {
      Thread::Current::SleepRelative(ZX_USEC(100));
    }
  }

  // Mutate threads as they run.
  for (int i = 0; i < 10'000; i++) {
    for (size_t j = 0; j < threads.size(); j++) {
      const cpu_mask_t affinity = static_cast<cpu_mask_t>(i + j) & Scheduler::PeekActiveMask();
      if (affinity) {
        threads[j].thread->SetSoftCpuAffinity(affinity);
      }
    }
    Thread::Current::SleepRelative(ZX_USEC(100));
  }

  // Join threads.
  for (auto& thread : threads) {
    // Inform the thread to stop.
    thread.should_stop.store(true);

    // Wait for it to finish.
    int ret;
    zx_status_t result = thread.thread->Join(&ret, Deadline::after_mono(ZX_SEC(5)).when());
    if (result != ZX_OK) {
      // If the thread has not completed in 5 seconds, it is likely that the
      // thread has hung for an unknown reason.
      //
      // TODO(https://fxbug.dev/42158849): We are currently seeing some flakes in CI/CQ
      // that cannot be reproduced locally. Once resolved, this additional
      // logging can be removed.
      thread.thread->Dump(/*full=*/true);
      Backtrace bt;
      thread.thread->GetBacktrace(bt);
      bt.Print();

      ZX_ASSERT_MSG(result == ZX_OK, "Failed to join worker thread: %d\n", result);
    }
  }

  END_TEST;
}

bool set_migrate_fn_stress_test() {
  BEGIN_TEST;

  // Get number of CPUs in the system.
  int active_cpus = ktl::popcount(Scheduler::PeekActiveMask());
  printf("Found %d active CPU(s)\n", active_cpus);
  if (active_cpus <= 1) {
    printf("Test can only proceed with multiple active CPUs.\n");
    return true;
  }

  struct Worker {
    Thread* thread;
    ktl::atomic<bool> should_stop = false;

    static void Migrate(Thread* thread, Thread::MigrateStage stage) {}

    static int Body(void* arg) {
      Worker* state = static_cast<Worker*>(arg);

      while (!state->should_stop.load(ktl::memory_order_relaxed)) {
        // Spin or sleep for 100us.
        const zx_duration_mono_t delay = ZX_USEC(100);
        if (rand() & 1) {
          const zx_instant_mono_t spin_end = zx_time_add_duration(current_mono_time(), delay);
          while (current_mono_time() < spin_end) {
            arch::Yield();
          }
        } else {
          Thread::Current::SleepRelative(delay);
        }
      }

      return 0;
    }
  };

  // Create threads.
  const int num_threads = active_cpus * 2;
  fbl::AllocChecker ac;
  fbl::Array<Worker> threads(new (&ac) Worker[num_threads], num_threads);
  ZX_ASSERT(ac.check());

  for (int i = 0; i < num_threads; i++) {
    threads[i].thread = Thread::Create("set_migrate_fn_stress_test worker", Worker::Body,
                                       &threads[i], DEFAULT_PRIORITY);
    ASSERT_NONNULL(threads[i].thread, "Thread::Create failed.");
    threads[i].thread->Resume();
  }

  // Periodically set/unset the migrate fn of the first thread.
  for (int i = 0; i < 10000; i++) {
    Thread::Current::SleepRelative(ZX_USEC(100));
    threads[0].thread->SetMigrateFn(i & 1 ? nullptr : Worker::Migrate);
  }

  // Join threads.
  for (auto& thread : threads) {
    // Inform the thread to stop.
    thread.should_stop.store(true);

    // Wait for it to finish.
    while (true) {
      int ret;
      zx_status_t result = thread.thread->Join(&ret, Deadline::after_mono(ZX_SEC(5)).when());
      if (result == ZX_OK) {
        break;
      }

      // If the thread has not completed in 5 seconds, either the thread has
      // hung for an unknown reason or there may have been lost time due to
      // virtualization. Keep trying until the overall test times out to
      // differentiate between an indefinite hang and virtualization time
      // loss.
      //
      // TODO(https://fxbug.dev/42158849): We are currently seeing some flakes
      // in CI/CQ that cannot be reproduced locally. Once resolved, this
      // additional logging can be removed.
      thread.thread->Dump(/*full=*/true);
      Backtrace bt;
      thread.thread->GetBacktrace(bt);
      bt.Print();
    }
  }

  END_TEST;
}

bool set_context_switch_fn_test() {
  BEGIN_TEST;

  // The worker thread will attempt to context switch.
  auto worker_body = [](void* arg) -> int { return Thread::Current::SleepRelative(ZX_MSEC(1)); };
  Thread* worker =
      Thread::Create("set_context_switch_fn_test_worker", worker_body, nullptr, DEFAULT_PRIORITY);
  ASSERT_NONNULL(worker, "thread_create failed.");

  // Set the context switch function, and begin execution.
  int context_switches = 0;
  worker->SetContextSwitchFn([&context_switches] { ++context_switches; });
  worker->Resume();

  // Wait for the worker thread to test itself.
  int worker_retcode;
  ASSERT_EQ(worker->Join(&worker_retcode, ZX_TIME_INFINITE), ZX_OK, "Failed to join thread.");
  EXPECT_EQ(worker_retcode, ZX_OK, "Worker thread failed.");
  // There are at least 4 context switches that occur as part of this test:
  // 1. When the thread is initially scheduled.
  // 2. When the thread is blocked for sleep.
  // 3. When the thread is rescheduled.
  // 4. When the thread exits.
  //
  // Additionally context switches may occur to additional scheduling events
  // during regular system operation.
  EXPECT_GE(context_switches, 4, "Context switch function was not called at least 4 times.");

  END_TEST;
}

bool scoped_allocation_disabled_test() {
  BEGIN_TEST;

  EXPECT_TRUE(Thread::Current::memory_allocation_state().IsEnabled());

  Thread::Current::memory_allocation_state().Disable();
  EXPECT_FALSE(Thread::Current::memory_allocation_state().IsEnabled());
  Thread::Current::memory_allocation_state().Enable();
  EXPECT_TRUE(Thread::Current::memory_allocation_state().IsEnabled());

  {
    EXPECT_TRUE(Thread::Current::memory_allocation_state().IsEnabled());
    {
      ScopedMemoryAllocationDisabled smad;
      EXPECT_FALSE(Thread::Current::memory_allocation_state().IsEnabled());
      {
        ScopedMemoryAllocationDisabled nested_smad;
        EXPECT_FALSE(Thread::Current::memory_allocation_state().IsEnabled());
      }
      EXPECT_FALSE(Thread::Current::memory_allocation_state().IsEnabled());
    }
    EXPECT_TRUE(Thread::Current::memory_allocation_state().IsEnabled());
  }

  END_TEST;
}

bool backtrace_static_method_test() {
  BEGIN_TEST;

  Backtrace bt;
  Thread::Current::GetBacktrace(bt);

  // Make sure we have at least one frames worth.
  ASSERT_GT(bt.size(), 1u);

  bt.reset();
  Thread::Current::GetBacktrace(reinterpret_cast<vaddr_t>(__GET_FRAME(0)), bt);
  ASSERT_GT(bt.size(), 1u);

  // See that we don't crash.
  bt.reset();
  Thread::Current::GetBacktrace(0, bt);
  ASSERT_EQ(bt.size(), 0u);

  END_TEST;
}

bool backtrace_instance_method_test() {
  BEGIN_TEST;

  struct Args {
    ktl::atomic<bool> running{false};
    Event event;
  } args;

  thread_start_routine helper = [](void* _args) -> int {
    auto args = reinterpret_cast<Args*>(_args);
    args->running.store(true);

    // Busy wait.
    while (args->running.load()) {
      arch::Yield();
    }

    // Block.
    args->event.Wait();

    return 0;
  };

  Thread* t = Thread::Create("backtrace helper", helper, &args, DEFAULT_PRIORITY);
  ASSERT_NONNULL(t);
  auto cleanup = fit::defer([&]() { t->Join(nullptr, ZX_TIME_INFINITE); });

  t->Resume();

  // Wait for the thread to run.
  while (!args.running.load()) {
    arch::Yield();
  }

  // Can't take the backtrace of a running thread.
  Backtrace bt;
  t->GetBacktrace(bt);
  ASSERT_EQ(0u, bt.size());

  // Tell the thread to go block.
  args.running.store(false);

  // Wait for it to block.
  thread_state state = thread_state::THREAD_RUNNING;
  while (state != thread_state::THREAD_BLOCKED) {
    Thread::Current::SleepRelative(ZX_USEC(200));
    SingleChainLockGuard guard{IrqSaveOption, t->get_lock(),
                               CLT_TAG("backtrace_instance_method_test")};
    state = t->state();
  }

  // See that we can backtrace a blocked thread.
  bt.reset();
  t->GetBacktrace(bt);
  ASSERT_GT(bt.size(), 0u);

  args.event.Signal();

  int result = -1;
  cleanup.cancel();
  ASSERT_EQ(ZX_OK, t->Join(&result, ZX_TIME_INFINITE));
  ASSERT_EQ(ZX_OK, result);

  END_TEST;
}

// ScopedMemoryStall is currently meant to be a no-op when executed in a kernel-only thread.
// This test verifies that it, indeed, does not modify the thread's state.
bool scoped_memory_stall_test() {
  BEGIN_TEST;

  Thread* const current_thread = Thread::Current::Get();

  // Verify previous state.
  {
    AnnotatedAutoPreemptDisabler preempt_disable;
    ASSERT_EQ(ThreadStallState::IgnoredKernelOnly, current_thread->memory_stall_state());
  }

  // Enter a memory stall and verify that the state has not changed.
  {
    ScopedMemoryStall scoped_memory_stall;
    AnnotatedAutoPreemptDisabler preempt_disable;
    ASSERT_EQ(ThreadStallState::IgnoredKernelOnly, current_thread->memory_stall_state());
  }

  // After exiting the memory stall, verify that the state has not changed.
  {
    AnnotatedAutoPreemptDisabler preempt_disable;
    ASSERT_EQ(ThreadStallState::IgnoredKernelOnly, current_thread->memory_stall_state());
  }

  END_TEST;
}

}  // namespace

struct TaskRuntimeStatsTests {
  static bool thread_runtime_test() {
    BEGIN_TEST;

    const zx_duration_mono_ticks_t kCpuTicks = 10, kQueueTicks = 20;
    TaskRuntimeStats trs;
    ThreadRuntimeStats stats;
    ThreadRuntimeStats::ReadResult read_res = stats.Read();
    const ThreadRuntimeStats::ThreadStats& sched_stats = read_res.stats;

    EXPECT_EQ(THREAD_INITIAL, sched_stats.current_state);
    EXPECT_EQ(0, sched_stats.state_change_ticks);
    EXPECT_EQ(0, sched_stats.total_running_ticks);
    EXPECT_EQ(0, sched_stats.total_ready_ticks);

    trs = stats.GetCompensatedTaskRuntimeStats();
    EXPECT_EQ(0, trs.cpu_ticks);
    EXPECT_EQ(0, trs.queue_ticks);
    EXPECT_EQ(0, trs.page_fault_ticks);
    EXPECT_EQ(0, trs.lock_contention_ticks);

    auto DelayTicks = [](zx_duration_mono_ticks_t ticks) {
      zx_instant_mono_ticks_t deadline = current_mono_ticks() + ticks;
      do {
        arch::Yield();
      } while (current_mono_ticks() < deadline);
    };

    // Test that runtime is calculated as a function of the stats and the current time spent queued.
    // When the state is set to THREAD_READY, TotalRuntime calculates queue_ticks as:
    //   runtime.queue_ticks + (current_mono_ticks() - state_change_ticks)
    //
    stats.Update(thread_state::THREAD_READY, ThreadRuntimeStats::IrqSave);

    // Spin for at least kQueueTicks, so we know that the projected amount of
    // time has to be >= kQueueTicks.
    DelayTicks(kQueueTicks);

    read_res = stats.Read();
    EXPECT_EQ(thread_state::THREAD_READY, sched_stats.current_state);
    EXPECT_NE(0, sched_stats.state_change_ticks);
    EXPECT_EQ(0, sched_stats.total_running_ticks);
    EXPECT_EQ(0, sched_stats.total_ready_ticks);  // Nothing accumulates into queue ticks until we
                                                  // leave the READY state.

    trs = stats.GetCompensatedTaskRuntimeStats();
    EXPECT_LE(kQueueTicks, trs.queue_ticks);
    EXPECT_EQ(0, trs.cpu_ticks);
    EXPECT_EQ(0, trs.page_fault_ticks);
    EXPECT_EQ(0, trs.lock_contention_ticks);

    // Test that runtime is calculated as a function of the stats and the
    // current time spent running. When the state is set to THREAD_RUNNING,
    // TotalRuntime calculates cpu_ticks as:
    //
    // runtime.cpu_ticks + (current_mono_ticks() - state_change_ticks)
    //
    // Start by changing state to running.
    stats.Update(thread_state::THREAD_RUNNING, ThreadRuntimeStats::IrqSave);

    // When we read the non-compensated stats, the total accumulated queue time
    // has to be at least the compensated queue time before we switched to
    // running.  The accumulated CPU time should still be zero, since we have
    // never been in the running state, then switched out.
    const zx_duration_mono_ticks_t queue_ticks_lower_bound = trs.queue_ticks;

    read_res = stats.Read();
    EXPECT_EQ(thread_state::THREAD_RUNNING, sched_stats.current_state);
    EXPECT_NE(0, sched_stats.state_change_ticks);
    EXPECT_EQ(0, sched_stats.total_running_ticks);
    EXPECT_LE(queue_ticks_lower_bound, sched_stats.total_ready_ticks);

    // Spend some time in this state and make sure that only the compensated
    // stat for the state we are in is advancing.
    DelayTicks(kCpuTicks);
    trs = stats.GetCompensatedTaskRuntimeStats();
    EXPECT_LE(kCpuTicks, trs.cpu_ticks);
    EXPECT_EQ(sched_stats.total_ready_ticks, trs.queue_ticks);
    EXPECT_EQ(0, trs.page_fault_ticks);
    EXPECT_EQ(0, trs.lock_contention_ticks);

    // Add other times.
    const zx_duration_mono_ticks_t kPageFaultTicks = 30;
    const zx_duration_mono_ticks_t kLockContentionTicks = 40;
    const TaskRuntimeStats prev_trs = trs;

    stats.AddPageFaultTicks(kPageFaultTicks);
    trs = stats.GetCompensatedTaskRuntimeStats();
    EXPECT_LE(prev_trs.cpu_ticks, trs.cpu_ticks);
    EXPECT_EQ(prev_trs.queue_ticks, trs.queue_ticks);
    EXPECT_EQ(kPageFaultTicks, trs.page_fault_ticks);
    EXPECT_EQ(0, trs.lock_contention_ticks);

    stats.AddLockContentionTicks(kLockContentionTicks);
    trs = stats.GetCompensatedTaskRuntimeStats();
    EXPECT_LE(prev_trs.cpu_ticks, trs.cpu_ticks);
    EXPECT_EQ(prev_trs.queue_ticks, trs.queue_ticks);
    EXPECT_EQ(kPageFaultTicks, trs.page_fault_ticks);
    EXPECT_EQ(kLockContentionTicks, trs.lock_contention_ticks);

    END_TEST;
  }
};

UNITTEST_START_TESTCASE(thread_tests)
UNITTEST("set_affinity_self_test", set_affinity_self_test)
UNITTEST("set_affinity_other_test", set_affinity_other_test)
UNITTEST("thread_last_cpu_new_thread", thread_last_cpu_new_thread)
UNITTEST("thread_last_cpu_running_thread", thread_last_cpu_running_thread)
UNITTEST("thread_empty_soft_affinity_mask", thread_empty_soft_affinity_mask)
UNITTEST("thread_conflicting_soft_and_hard_affinity", thread_conflicting_soft_and_hard_affinity)
UNITTEST("set_migrate_fn_test", set_migrate_fn_test)
UNITTEST("set_migrate_ready_threads_test", set_migrate_ready_threads_test)
UNITTEST("migrate_unpinned_threads_test", migrate_unpinned_threads_test)
UNITTEST("migrate_stress_test", migrate_stress_test)
UNITTEST("set_migrate_fn_stress_test", set_migrate_fn_stress_test)
UNITTEST("set_context_switch_fn", set_context_switch_fn_test)
UNITTEST("scoped_allocation_disabled_test", scoped_allocation_disabled_test)
UNITTEST("backtrace_static_method_test", backtrace_static_method_test)
UNITTEST("backtrace_instance_method_test", backtrace_instance_method_test)
UNITTEST("scoped_memory_stall_test", scoped_memory_stall_test)
UNITTEST("thread_runtime_test", TaskRuntimeStatsTests::thread_runtime_test)
UNITTEST_END_TESTCASE(thread_tests, "thread", "thread tests")
