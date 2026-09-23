// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/intrin.h>
#include <lib/control-region.h>
#include <lib/unittest/unittest.h>

#include <arch/interrupt.h>
#include <fbl/algorithm.h>
#include <kernel/thread.h>

namespace {

// A basic stress test for ControlRegion.
//
// Spawns several threads, then has them all repeatedly enter and leave the control region.
bool control_region_stress() {
  BEGIN_TEST;

  static constexpr size_t kNumThreads = 4;

  class TestState {
   public:
    static int ThreadEntry(void* arg) {
      auto* state = reinterpret_cast<TestState*>(arg);
      state->Run();
      return 0;
    }

    void Stop() { done_.store(true, ktl::memory_order_release); }

   private:
    // Each thread executes this function once.
    void Run() {
      // Only used for diagnostic output.
      size_t count_entered = 0;
      size_t count_closed = 0;

      region_.Register();
      WaitForAll();

      while (KeepRunning()) {
        // Enter the region.
        {
          InterruptDisableGuard irqd;
          if (!region_.TryEnter()) {
            region_.CloseEnter();
            ++count_closed;
          }
        }

        ++count_entered;

        // Leave the region.
        {
          InterruptDisableGuard irqd;
          if (!region_.TryLeave()) {
            region_.OpenLeave();
          }
        }
      }

      // Note, we don't call unregister here because we cannot guarantee that there are no other
      // threads within the control region.

      printf("counts: entered=%zu, closed=%zu\n", count_entered, count_closed);
    }

    // Spins until all other threads reach this point or the test is stopped.
    //
    // This method acts as a reusable synchronization barrier.
    void WaitForAll() {
      const uint64_t orig_gen = barrier_generation_.load(ktl::memory_order_acquire);
      uint64_t num_waiting = barrier_count_.fetch_add(1, ktl::memory_order_acq_rel);
      if (num_waiting + 1 == kNumThreads) {
        barrier_count_.store(0, ktl::memory_order_release);
        barrier_generation_.fetch_add(1, ktl::memory_order_acq_rel);
        return;
      }
      while (orig_gen == barrier_generation_.load(ktl::memory_order_acquire) && KeepRunning()) {
        arch::Yield();
      }
    }

    bool KeepRunning() const { return !done_.load(ktl::memory_order_acquire); }

    // Used to implement WaitForAll.  When a thread "arrives" (i.e. calls WaitForAll), it reads the
    // generation value, then increments the count.  If the caller is not the last to arrive, it
    // will spin until the generation value is bumped.  If the caller is the last to arrive
    // (i.e. has incremented the count to kNumThreads), it will reset the count to zero and bump the
    // generation, thereby releasing the other threads.
    ktl::atomic<size_t> barrier_generation_{0};
    ktl::atomic<size_t> barrier_count_{0};

    // Used to signal that the tests is ending and the threads should terminate.
    ktl::atomic<bool> done_{false};

    // The system under test.
    ControlRegion region_;
  };

  TestState state;

  // Create a bunch of threads.
  Thread* threads[kNumThreads];
  for (size_t i = 0; i < kNumThreads; ++i) {
    threads[i] = Thread::Create("cr-stress", TestState::ThreadEntry, &state, DEFAULT_PRIORITY);
    ASSERT_TRUE(threads[i]);
    threads[i]->Resume();
  }

  // Let them run for a bit before asking them to stop.
  Thread::Current::SleepRelative(ZX_SEC(1));
  state.Stop();

  // Join them.
  for (size_t i = 0; i < kNumThreads; ++i) {
    int result;
    EXPECT_OK(threads[i]->Join(&result, ZX_TIME_INFINITE));
    EXPECT_EQ(0, result);
  }

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(control_region_tests)
UNITTEST("stress", control_region_stress)
UNITTEST_END_TESTCASE(control_region_tests, "control-region", "control-region tests")
