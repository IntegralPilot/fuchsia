// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>
#include <src/performance/memory/sampler/instrumentation/poisson_sampler.h>

namespace memory_sampler {
namespace {
constexpr size_t kHighSamplingRateInterval = 1;
constexpr size_t kLowSamplingRateInterval = 1000;
constexpr size_t kSmallAllocation = 1;
constexpr size_t kLargeAllocation = 1000;
constexpr size_t kMediumAllocation = 100;

TEST(PoissonSamplerTest, SampleLargeAllocation) {
  class TestSampleIntervalGenerator : public PoissonSampler::SampleIntervalGenerator {
   public:
    size_t GetNextSampleInterval(size_t) override { return kHighSamplingRateInterval; }
  };

  // Create a sampler with a high sampling rate.
  PoissonSampler sampler{kHighSamplingRateInterval,
                         std::make_unique<TestSampleIntervalGenerator>()};

  // Query the sampler with a large allocation.
  EXPECT_TRUE(sampler.ShouldSampleAllocation(kLargeAllocation));
}

TEST(PoissonSamplerTest, DontSampleSmallAllocation) {
  class TestSampleIntervalGenerator : public PoissonSampler::SampleIntervalGenerator {
   public:
    size_t GetNextSampleInterval(size_t) override { return kLowSamplingRateInterval; }
  };
  // Create a sampler with a low sampling rate.
  PoissonSampler sampler{kLowSamplingRateInterval, std::make_unique<TestSampleIntervalGenerator>()};

  // Query the sampler with a small allocation.
  EXPECT_FALSE(sampler.ShouldSampleAllocation(kSmallAllocation));
}

TEST(PoissonSamplerTest, SampleProbabilityIncreases) {
  class TestSampleIntervalGenerator : public PoissonSampler::SampleIntervalGenerator {
   public:
    size_t GetNextSampleInterval(size_t) override { return kLowSamplingRateInterval; }
  };
  // Create a sampler with a low sampling rate.
  PoissonSampler sampler{kLowSamplingRateInterval, std::make_unique<TestSampleIntervalGenerator>()};

  // Query the sampler with small allocations until it samples.
  size_t allocated_bytes = 0;
  while (allocated_bytes < kLowSamplingRateInterval - kMediumAllocation) {
    EXPECT_FALSE(sampler.ShouldSampleAllocation(kMediumAllocation));
    allocated_bytes += kMediumAllocation;
  }

  EXPECT_TRUE(sampler.ShouldSampleAllocation(kMediumAllocation));
}
}  // namespace
}  // namespace memory_sampler
