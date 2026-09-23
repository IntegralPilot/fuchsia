// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "simple_rx_buffer_estimator.h"

#include <zircon/assert.h>

#include <algorithm>
#include <limits>

#include "log.h"
namespace {
// A lower smoothing factor to have a longer memory.
constexpr double kDefaultAlpha = 0.2;
// The network device benchmarks shows that maximum Rx return latency is
// about 0.25ms, which is measured on a CPU constrained platform, other
// products are significantly lower than that. 1 millisecond is 4 times
// that. With this value and a 512 total buffers, we can handle 512,000
// packet per second traffic, which is > 6 gbps. So this budget should
// be enough in most cases.
constexpr zx::duration kDefaultDelayBudget = zx::msec(1);
// The netdevice client uses the same sample rate.
constexpr zx::duration kDefaultSampleInterval = zx::sec(1);
// An empirically chosen value that is large but does not cause
// buffer starvation.
constexpr double kDefaultVarianceThreshold = 2.0;
}  // namespace

namespace network::internal {

SimpleRxBufferEstimator::SimpleRxBufferEstimator(double alpha, zx::duration delay_budget,
                                                 zx::duration sample_interval,
                                                 double variance_threshold)
    : alpha_fixed_(static_cast<uint64_t>(alpha * static_cast<double>(1ull << kFracBits))),
      delay_budget_(delay_budget),
      sample_interval_(sample_interval),
      variance_threshold_fixed_(
          static_cast<uint64_t>(variance_threshold * static_cast<double>(1ull << kFracBits))) {
  ZX_ASSERT(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer_) == ZX_OK);
}

SimpleRxBufferEstimator::~SimpleRxBufferEstimator() {
  if (timer_.is_valid()) {
    if (zx_status_t status = timer_.cancel(); status != ZX_OK) {
      LOGF_ERROR("failed to cancel the timer for Rx buffer management: %s",
                 zx_status_get_string(status));
    }
  }
}

void SimpleRxBufferEstimator::Update(uint64_t new_packets_per_second) {
  constexpr uint64_t BETA = (1ull << (kFracBits - 1));

  uint64_t new_packets_per_second_scaled = new_packets_per_second << kFracBits;
  uint64_t new_deviation_scaled =
      std::max(new_packets_per_second_scaled, packets_per_second_scaled_) -
      std::min(new_packets_per_second_scaled, packets_per_second_scaled_);
  deviation_scaled_ =
      ((BETA * new_deviation_scaled) + (((1ull << kFracBits) - BETA) * deviation_scaled_)) >>
      kFracBits;
  packets_per_second_scaled_ =
      ((alpha_fixed_ * new_packets_per_second_scaled) +
       (((1ull << kFracBits) - alpha_fixed_) * packets_per_second_scaled_)) >>
      kFracBits;
}

uint16_t SimpleRxBufferEstimator::CalculateTargetBuffers() const {
  uint64_t pps = packets_per_second_scaled_ >> kFracBits;
  int64_t delay_ns = delay_budget_.to_nsecs();
  uint64_t calculated = (pps * delay_ns) / 1'000'000'000LL;
  return static_cast<uint16_t>(
      std::min(calculated, static_cast<uint64_t>(std::numeric_limits<uint16_t>::max())));
}

uint16_t SimpleRxBufferEstimator::NeedImmediateBuffers(uint64_t pps) {
  uint64_t pps_scaled = pps << kFracBits;
  if (pps_scaled > packets_per_second_scaled_ &&
      (pps_scaled - packets_per_second_scaled_) >
          ((variance_threshold_fixed_ * deviation_scaled_) >> kFracBits)) {
    packets_per_second_scaled_ = pps_scaled;
    return CalculateTargetBuffers();
  }
  return 0;
}

zx_status_t SimpleRxBufferEstimator::RearmTimer(const zx::port& port, uint64_t key) {
  if (!timer_.is_valid()) {
    return ZX_ERR_BAD_HANDLE;
  }
  if (zx_status_t status = timer_.set(zx::deadline_after(sample_interval_), zx::sec(0));
      status != ZX_OK) {
    LOGF_ERROR("%s timer set failed %s", __FUNCTION__, zx_status_get_string(status));
    return status;
  }
  if (zx_status_t status = timer_.wait_async(port, key, ZX_TIMER_SIGNALED, 0); status != ZX_OK) {
    LOGF_ERROR("%s timer wait_async failed %s", __FUNCTION__, zx_status_get_string(status));
    ZX_ASSERT(timer_.cancel() == ZX_OK);
    return status;
  }
  return ZX_OK;
}

void SimpleRxBufferEstimator::CancelTimer(const zx::port& port, uint64_t key) {
  if (!timer_.is_valid()) {
    return;
  }
  if (zx_status_t status = timer_.cancel(); status != ZX_OK) {
    LOGF_ERROR("failed to cancel the timer for Rx buffer management: %s",
               zx_status_get_string(status));
  }
  if (zx_status_t status = port.cancel(timer_, key);
      status != ZX_OK && status != ZX_ERR_NOT_FOUND) {
    LOGF_ERROR("failed to cancel port wait for Rx buffer management: %s",
               zx_status_get_string(status));
  }
}

std::optional<SimpleRxBufferEstimator> RxBufferEstimatorFromFidl(
    const netdriver::RxBufferManagement& management) {
  if (management.Which() == netdriver::RxBufferManagement::Tag::kSimple) {
    const auto& simple = management.simple().value();
    double alpha = simple.alpha();
    if (alpha == 0.0) {
      alpha = kDefaultAlpha;
    }
    if (alpha < 0.0 || alpha > 1.0) {
      LOGF_ERROR("invalid alpha %lf, expected in (0, 1]", alpha);
    }
    zx::duration delay_budget = zx::duration(simple.delay_budget());
    if (delay_budget == zx::duration(0)) {
      delay_budget = kDefaultDelayBudget;
    }
    zx::duration sample_interval = zx::duration(simple.sample_interval());
    if (sample_interval == zx::duration(0)) {
      sample_interval = kDefaultSampleInterval;
    }
    double variance_threshold = simple.variance_threshold();
    if (variance_threshold == 0.0) {
      variance_threshold = kDefaultVarianceThreshold;
    }
    return SimpleRxBufferEstimator(alpha, delay_budget, sample_interval, variance_threshold);
  }
  ZX_ASSERT_MSG(management.Which() == netdriver::RxBufferManagement::Tag::kStatic,
                "unknown Rx buffer management type");
  return std::nullopt;
}

}  // namespace network::internal
