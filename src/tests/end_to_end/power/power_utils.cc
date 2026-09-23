// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <chrono>
#include <thread>
#include <vector>

namespace power {

void intenseComputation(uint64_t duration) {
  std::chrono::high_resolution_clock::time_point start = std::chrono::high_resolution_clock::now();
  std::chrono::high_resolution_clock::time_point end = start + std::chrono::seconds(duration);
  double x __attribute__((unused)) = 0.0;

  while (std::chrono::high_resolution_clock::now() < end) {
    x += std::sin(0.0001) * std::exp(0.0001);
  }
}

void intenseComputationOnAllCores(uint64_t duration) {
  const uint32_t numThreads = std::thread::hardware_concurrency();

  std::vector<std::thread> threads;
  for (uint32_t i = 0; i < numThreads; ++i) {
    threads.emplace_back([duration]() { intenseComputation(duration); });
  }

  for (uint32_t i = 0; i < numThreads; ++i) {
    threads[i].join();
  }
}

void idleThread(uint64_t duration) {
  std::this_thread::sleep_for(
      std::chrono::seconds(static_cast<std::chrono::seconds::rep>(duration)));
}

}  // namespace power
