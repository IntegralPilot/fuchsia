// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <pthread.h>

#include <vector>

#include <perftest/perftest.h>

namespace {

// Helper entry-point for pthreads that just waits on the supplied barrier.
void* WaitOnBarrier(void* arg) {
  FX_CHECK(pthread_barrier_wait(static_cast<pthread_barrier_t*>(arg)) <= 0);
  return nullptr;
}

// A no-op helper entry-point for pthreads in this fixture.
void* ExitImmediately(void* arg) { return nullptr; }

// Benchmark for creating and joining on a pthread with a body that does nothing. Can create
// |existing| amount of pthreads, which will be left blocked waiting on a barrier, to determine if
// inactive pthreads negatively impact create/join performance.
bool PThreadCreateAndJoinTest(perftest::RepeatState* state, int existing) {
  // Initialize a barrier that expects all |existing| threads plus ourselves to wait on.
  pthread_barrier_t barrier;
  FX_CHECK(pthread_barrier_init(&barrier, nullptr, existing + 1) == 0);

  // Create any requested existing threads and have them wait on the barrier.
  std::vector<pthread_t> existing_threads;
  existing_threads.resize(existing);
  for (auto& thread : existing_threads) {
    FX_CHECK(pthread_create(&thread, nullptr, WaitOnBarrier, &barrier) == 0);
  }

  while (state->KeepRunning()) {
    pthread_t thread;
    FX_CHECK(pthread_create(&thread, nullptr, ExitImmediately, nullptr) == 0);
    FX_CHECK(pthread_join(thread, nullptr) == 0);
  }

  // Wait on the barrier ourselves to release all the other threads so we can join them.
  FX_CHECK(pthread_barrier_wait(&barrier) <= 0);
  for (auto& thread : existing_threads) {
    FX_CHECK(pthread_join(thread, nullptr) == 0);
  }
  return true;
}

void RegisterTests() {
  perftest::RegisterTest("PThreadCreateAndJoinTest", PThreadCreateAndJoinTest, 0);
  perftest::RegisterTest("PThreadCreateAndJoinTest/100Existing", PThreadCreateAndJoinTest, 100);
  perftest::RegisterTest("PThreadCreateAndJoinTest/1000Existing", PThreadCreateAndJoinTest, 1000);
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
