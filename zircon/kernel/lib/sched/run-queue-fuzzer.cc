// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/sched/run-queue.h>
#include <lib/sched/thread-base.h>
#include <zircon/time.h>

#include <memory>
#include <vector>

#include <fuzzer/FuzzedDataProvider.h>

#include "test-thread.h"

//
// This file defines a simple fuzzer of sched::RunQueue, simulating the ongoing
// scheduling of threads.
//

namespace {

using Duration = sched::Duration;
using FlexibleWeight = sched::FlexibleWeight;
using Time = sched::Time;

enum class Action : uint8_t {
  // Allocate and queue a new thread.
  kQueueNewThread,

  // If there is a thread currently running, set it as blocked.
  kBlockCurrentThread,

  // Unblock the last blocked thread, if any.
  kUnblockThread,

  // Mandated by FuzzedDataProvider::ConsumeEnum.
  kMaxValue,
};

Time ConsumeTime(FuzzedDataProvider& provider) {
  return Time{provider.ConsumeIntegral<zx_instant_mono_t>()};
}

FlexibleWeight ConsumeFlexibleWeight(FuzzedDataProvider& provider, FlexibleWeight min) {
  return FlexibleWeight{
      provider.ConsumeIntegralInRange<int64_t>(min.raw_value(), FlexibleWeight::Max().raw_value())};
}

Duration ConsumeDuration(FuzzedDataProvider& provider, Duration min, Duration max) {
  ZX_ASSERT(min >= 0);
  ZX_ASSERT(max >= min);
  return Duration{
      provider.ConsumeIntegralInRange<zx_duration_mono_t>(min.raw_value(), max.raw_value())};
}

TestThread* AllocateNewThread(FuzzedDataProvider& provider,
                              std::vector<std::unique_ptr<TestThread>>& threads) {
  Time start = ConsumeTime(provider);
  Duration period = ConsumeDuration(provider, Duration{1}, (Time::Max() - start) + Time{1});
  Duration firm_capacity = ConsumeDuration(provider, Duration{0}, period);
  FlexibleWeight flexible_weight =
      ConsumeFlexibleWeight(provider, FlexibleWeight{firm_capacity == 0 ? 1 : 0});
  threads.emplace_back(std::make_unique<TestThread>(
      sched::BandwidthParameters{
          .period = period,
          .firm_capacity = firm_capacity,
          .flexible_weight = flexible_weight,
      },
      start));
  return threads.back().get();
}

}  // namespace

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  FuzzedDataProvider provider(data, size);

  std::vector<std::unique_ptr<TestThread>> threads;
  std::vector<TestThread*> blocked;
  Time now = ConsumeTime(provider);
  TestThread* current = nullptr;
  sched::RunQueue<TestThread> queue;
  while (now < Time::Max() && provider.remaining_bytes() > 0) {
    switch (provider.ConsumeEnum<Action>()) {
      case Action::kQueueNewThread:
        queue.Queue(*AllocateNewThread(provider, threads), now);
        break;
      case Action::kBlockCurrentThread:
        if (current) {
          current->set_state(sched::ThreadState::kBlocked);
          blocked.push_back(current);
        }
        break;
      case Action::kUnblockThread:
        if (!blocked.empty()) {
          queue.Queue(*blocked.back(), now);
          blocked.pop_back();
        }
        break;
      case Action::kMaxValue:
        break;
    }

    ZX_ASSERT(current == queue.current_thread());
    if (current) {
      ZX_ASSERT(!current->IsQueued());
      ZX_ASSERT(current->start() <= now);
    }

    auto [next, preemption] = queue.SelectNextThread(now);

    ZX_ASSERT(current == next || !current || current->IsQueued());

    if (next) {
      ZX_ASSERT(!next->IsQueued());
      ZX_ASSERT(next->start() <= now);
      ZX_ASSERT(now < preemption);
      ZX_ASSERT(preemption <= next->finish());

      next->Tick(preemption - now);
    }

    now = preemption;
    current = next;
  }

  return 0;
}
