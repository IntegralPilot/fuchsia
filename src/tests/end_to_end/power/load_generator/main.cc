// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <thread>
#include <vector>

#include "src/tests/end_to_end/power/load_generator/params.h"

void GenerateLoad(std::chrono::duration<int64_t, std::milli> duration) {
  const auto load_generator = [](std::chrono::duration<int64_t, std::milli> duration) {
    using time_point = std::chrono::high_resolution_clock::time_point;
    time_point start = std::chrono::high_resolution_clock::now();
    time_point end = start + duration;
    double x __attribute__((unused)) = 0.0;

    while (std::chrono::high_resolution_clock::now() < end) {
      x += std::sin(0.0001) * std::exp(0.0001);
    }
  };

  std::vector<std::thread> threads;
  for (size_t i = 0; i < std::thread::hardware_concurrency(); i++) {
    threads.emplace_back(load_generator, duration);
  }
  std::for_each(threads.begin(), threads.end(), [](std::thread& t) { t.join(); });
}

void GenerateLoadPattern(const std::vector<uint64_t>& pattern) {
  auto it = pattern.begin();
  while (it != pattern.end()) {
    // Load
    FX_LOGS(INFO) << "Generating 100% CPU Usage for " << *it << " milliseconds";
    GenerateLoad(std::chrono::milliseconds(*it++));

    // Sleep
    if (it == pattern.end()) {
      break;
    }
    FX_LOGS(INFO) << "Sleeping for " << *it << " milliseconds";
    std::this_thread::sleep_for(std::chrono::milliseconds(*it++));
  }
}

int main() {
  auto params = params::Config::TakeFromStartupHandle();
  GenerateLoadPattern(params.load_pattern());
}
