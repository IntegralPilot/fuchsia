// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "poisson_sampler.h"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <random>

namespace {
class NonDeterministicSampleIntervalGenerator final
    : public memory_sampler::PoissonSampler::SampleIntervalGenerator {
 public:
  // We sample with a Poisson process, with constant average sampling
  // interval. This follows the exponential probability distribution with
  // parameter λ = 1/interval where |interval| is the average number of bytes
  // between samples.
  // Let u be a uniformly distributed random number (0,1], then
  // next_sample = -ln(u) / λ
  //
  // Note: This formula is derived from inverse transform sampling, by
  // computing the inverse cumulative distribution to get a random
  // number following an arbitrary distribution from a uniform random
  // number generator.
  size_t GetNextSampleInterval(size_t mean_interval) override {
    // UniformRandomDouble returns numbers [0,1). We use 1-uniform to
    // correct it to avoid a possible floating point exception from
    // taking the log of 0.
    double uniform = UniformRandomDouble();
    double value = -std::log(1 - uniform) * static_cast<double>(mean_interval);
    double min_value = sizeof(intptr_t);
    // We limit the upper bound of a sample interval to make sure we don't have
    // huge gaps in the sampling stream. Probability of the upper bound gets hit
    // is exp(-20) ~ 2e-9, so it should not skew the distribution.
    double max_value = static_cast<double>(mean_interval * 20);
    return static_cast<size_t>(std::clamp(value, min_value, max_value));
  }

 private:
  // Random engine used to run the uniform real distribution.
  //
  // Note: We use a hardware RNG only for seeding; we expect a PRNG to
  // be good enough for the lifetime of the recorder.
  std::default_random_engine generator_{std::random_device{}()};
  std::uniform_real_distribution<double> distribution_{0, 1.0};
  // Draw a uniform random double from [0, 1).
  double UniformRandomDouble() { return distribution_(generator_); }
};

}  // namespace

namespace memory_sampler {
PoissonSampler::PoissonSampler(size_t mean_interval)
    : mean_interval_(mean_interval),
      sample_interval_generator_(std::make_unique<NonDeterministicSampleIntervalGenerator>()) {}

// The overall strategy to implement our Poisson process goes like
// this:
//
//   * Compute a sample interval.
//
//   * Keep track of the number of allocated bytes so far.
//
//   * Once this total has crossed the threshold defined by the
//   interval (which means that the "sampled byte" was part of this
//   allocation), compute the new threshold by getting a new interval,
//   then record this allocation.
bool PoissonSampler::ShouldSampleAllocation(size_t size) {
  if (!sampling_interval_initialized_) {
    sampling_interval_initialized_ = true;
    bytes_before_next_sample_ = sample_interval_generator_->GetNextSampleInterval(mean_interval_);
  }

  if (bytes_before_next_sample_ > size) {
    bytes_before_next_sample_ -= size;
    return false;
  }

  size_t spillover_bytes = size - bytes_before_next_sample_;
  size_t new_interval = 0;
  do {
    new_interval += sample_interval_generator_->GetNextSampleInterval(mean_interval_);
  } while (new_interval < spillover_bytes);

  bytes_before_next_sample_ = new_interval - spillover_bytes;
  return true;
}

}  // namespace memory_sampler
