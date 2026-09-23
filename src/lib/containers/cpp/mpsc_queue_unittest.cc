// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/containers/cpp/mpsc_queue.h"

#include <queue>
#include <set>
#include <thread>

#include <gtest/gtest.h>

namespace containers {

TEST(MpscQueueTest, Sanity) {
  MpscQueue<int> under_test;
  std::queue<int> expectation;
  const int kElements = 10;

  for (int i = 0; i < kElements; ++i) {
    under_test.Push(i);
    expectation.push(i);
  }

  for (int i = 0; i < kElements; ++i) {
    std::optional<int> maybe_elem = under_test.Pop();
    EXPECT_TRUE(maybe_elem.has_value());
    if (maybe_elem.has_value()) {
      EXPECT_EQ(expectation.front(), *maybe_elem);
      expectation.pop();
    }
  }
}

TEST(MpscQueueTest, TwoThreads) {
  MpscQueue<int> under_test;
  std::set<int> expectation;

  const int kElements = 100;
  for (int i = 0; i < kElements; ++i) {
    expectation.insert(i);
  }

  std::thread producer_thread([&under_test] {
    for (int i = 0; i < kElements; ++i) {
      under_test.Push(i);
    }
  });
  producer_thread.detach();

  int element_count = 0;
  while (element_count < kElements) {
    std::optional<int> maybe_elem = under_test.Pop();
    if (maybe_elem.has_value()) {
      ++element_count;
      expectation.erase(*maybe_elem);
    }
  }

  EXPECT_EQ(expectation.size(), 0u);
}

}  // namespace containers
