// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/unittest/unittest.h>
#include <platform.h>

#include <fbl/algorithm.h>
#include <kernel/brwlock.h>
#include <kernel/mp.h>
#include <ktl/atomic.h>

#include "tests.h"

#include <ktl/enforce.h>

// Use a delay spinner to create fine grained delays between 0 and 1msec
static void rand_delay() {
  int64_t end = current_mono_time() + (rand() % ZX_MSEC(1));
  do {
    Thread::Current::Yield();
  } while (current_mono_time() < end);
}

// Use a helper class for running tests so that worker threads and main thread all
// have easy access to shared state.
template <typename LockType>
class BrwLockTest {
 public:
  BrwLockTest() : state_(0), kill_(false) {}
  ~BrwLockTest() {}

  template <unsigned int readers, unsigned int writers>
  static bool RunTest() {
    BEGIN_TEST;

    BrwLockTest<LockType> test;
    Thread* reader_threads[readers];
    Thread* writer_threads[writers];

    const SchedulerState::BaseProfile old_bp = Thread::Current::Get()->SnapshotBaseProfile();

    // Run at high priority so that we can be validating what the other threads are doing.
    // Unless we are a uniprocessor, in which case we will just have to live with poor
    // testing. If we do boost priority then we need to make sure worker threads
    // don't ever get scheduled on our core, since we will never block and so they
    // will starve. This is currently a known starvation problem with the scheduler.
    cpu_mask_t worker_mask = mp_get_online_mask();
    if (lowest_cpu_set(worker_mask) != highest_cpu_set(worker_mask)) {
      mp_get_online_mask();
      Thread::Current::Get()->SetBaseProfile(SchedulerState::BaseProfile{HIGH_PRIORITY});
      cpu_mask_t pin_mask = cpu_num_to_mask(lowest_cpu_set(worker_mask));
      Thread::Current::Get()->SetCpuAffinity(pin_mask);
      worker_mask -= pin_mask;
    } else {
      Thread::Current::Get()->SetBaseProfile(SchedulerState::BaseProfile{DEFAULT_PRIORITY});
    }

    // Start threads
    for (auto& t : reader_threads) {
      t = Thread::Create(
          "reader worker",
          [](void* arg) -> int {
            static_cast<BrwLockTest*>(arg)->ReaderWorker();
            return 0;
          },
          &test, DEFAULT_PRIORITY);
      t->SetCpuAffinity(worker_mask);
      t->Resume();
    }
    for (auto& t : writer_threads) {
      t = Thread::Create(
          "writer worker",
          [](void* arg) -> int {
            static_cast<BrwLockTest*>(arg)->WriterWorker();
            return 0;
          },
          &test, DEFAULT_PRIORITY);
      t->SetCpuAffinity(worker_mask);
      t->Resume();
    }

    zx_instant_mono_t start = current_mono_time();
    zx_duration_mono_t duration = ZX_MSEC(300);
    while (current_mono_time() < start + duration) {
      uint32_t local_state = test.state_.load(ktl::memory_order_relaxed);
      uint32_t num_readers = local_state & 0xffff;
      uint32_t num_writers = local_state >> 16;
      EXPECT_LE(num_readers, readers, "Too many readers");
      EXPECT_TRUE(num_writers == 0 || num_writers == 1, "Too many writers");
      EXPECT_TRUE((num_readers == 0 && num_writers == 0) || num_writers > 0 || num_readers > 0,
                  "Readers and writers");
      Thread::Current::Yield();
    }

    // Like `Thread::Join` with an infinite timeout except that it will
    // periodically print a message that includes `description` so that it's
    // easier to debug hangs.
    //
    // Why an infinite timeout?  Because tests often run in virtualized/emulated
    // environments where any reasonable fixed timeout could can be exceeded.
    auto join = [](Thread* t, const char* description) -> zx_status_t {
      constexpr int64_t kTimeoutMs = 5000;
      while (true) {
        Deadline deadline = Deadline::after_mono(zx_duration_from_msec(kTimeoutMs));
        zx_status_t status = t->Join(nullptr, deadline.when());
        if (status != ZX_ERR_TIMED_OUT) {
          return status;
        }
        printf("failed to join %s after %ld msec, retrying\n", description, kTimeoutMs);
      }
    };

    // Shutdown all the threads. Validating they can shutdown is important
    // to ensure they didn't get stuck on the waitqueue and never woken up.
    test.kill_.store(true, ktl::memory_order_seq_cst);
    for (auto& t : reader_threads) {
      zx_status_t status = join(t, "Reader");
      EXPECT_EQ(status, ZX_OK, "Reader failed to join");
    }
    for (auto& t : writer_threads) {
      zx_status_t status = join(t, "Writer");
      EXPECT_EQ(status, ZX_OK, "Writer failed to join");
    }

    EXPECT_EQ(test.state_.load(ktl::memory_order_seq_cst), 0u, "Threads still holding lock");

    // Restore original base profile.
    Thread::Current::Get()->SetBaseProfile(old_bp);

    END_TEST;
  }

 private:
  void ReaderWorker() {
    while (!kill_.load(ktl::memory_order_relaxed)) {
      lock_.ReadAcquire();
      state_.fetch_add(1, ktl::memory_order_relaxed);
      Thread::Current::Yield();
      state_.fetch_sub(1, ktl::memory_order_relaxed);
      lock_.ReadRelease();
      rand_delay();
    }
  }

  void WriterWorker() {
    while (!kill_.load(ktl::memory_order_relaxed)) {
      lock_.WriteAcquire();
      state_.fetch_add(0x10000, ktl::memory_order_relaxed);
      Thread::Current::Yield();
      state_.fetch_sub(0x10000, ktl::memory_order_relaxed);
      lock_.WriteRelease();
      rand_delay();
    }
  }

  LockType lock_;
  ktl::atomic<uint32_t> state_;
  ktl::atomic<bool> kill_;
};

UNITTEST_START_TESTCASE(brwlock_tests)
// The number of threads to use for readers, writers and upgraders was chosen by manual
// instrumentation of the brwlock to see if all the different code paths were being hit.
UNITTEST("parallel readers(PI)", (BrwLockTest<BrwLockPi>::RunTest<8, 0>))
UNITTEST("single writer(PI)", (BrwLockTest<BrwLockPi>::RunTest<0, 4>))
UNITTEST("readers and writer(PI)", (BrwLockTest<BrwLockPi>::RunTest<4, 2>))
UNITTEST("parallel readers(No PI)", (BrwLockTest<BrwLockNoPi>::RunTest<8, 0>))
UNITTEST("single writer(No PI)", (BrwLockTest<BrwLockNoPi>::RunTest<0, 4>))
UNITTEST("readers and writer(No PI)", (BrwLockTest<BrwLockNoPi>::RunTest<4, 2>))
UNITTEST_END_TESTCASE(brwlock_tests, "brwlock", "brwlock tests")
