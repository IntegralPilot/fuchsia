// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_SIMPLE_RX_BUFFER_ESTIMATOR_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_SIMPLE_RX_BUFFER_ESTIMATOR_H_

#include <lib/zx/port.h>
#include <lib/zx/time.h>
#include <lib/zx/timer.h>

#include <optional>

#include "definitions.h"

namespace network::internal {

class SimpleRxBufferEstimator {
 public:
  static constexpr size_t kFracBits = 8;

  explicit SimpleRxBufferEstimator(double alpha = 0.2, zx::duration delay_budget = zx::msec(1),
                                   zx::duration sample_interval = zx::sec(1),
                                   double variance_threshold = 2.0);

  SimpleRxBufferEstimator(SimpleRxBufferEstimator&&) = default;
  SimpleRxBufferEstimator& operator=(SimpleRxBufferEstimator&&) = default;
  SimpleRxBufferEstimator(const SimpleRxBufferEstimator&) = delete;
  SimpleRxBufferEstimator& operator=(const SimpleRxBufferEstimator&) = delete;
  ~SimpleRxBufferEstimator();

  void Update(uint64_t new_packets_per_second);

  uint16_t CalculateTargetBuffers() const;

  uint16_t NeedImmediateBuffers(uint64_t pps);

  zx_status_t RearmTimer(const zx::port& port, uint64_t key);

  void CancelTimer(const zx::port& port, uint64_t key);

  double alpha() const {
    return static_cast<double>(alpha_fixed_) / static_cast<double>(1ull << kFracBits);
  }
  zx::duration delay_budget() const { return delay_budget_; }
  zx::duration sample_interval() const { return sample_interval_; }
  double variance_threshold() const {
    return static_cast<double>(variance_threshold_fixed_) / static_cast<double>(1ull << kFracBits);
  }

 private:
  uint64_t packets_per_second_scaled_{0};
  uint64_t deviation_scaled_{0};
  uint64_t alpha_fixed_{64};
  zx::duration delay_budget_{0};
  zx::duration sample_interval_{0};
  uint64_t variance_threshold_fixed_{512};
  zx::timer timer_;
};

std::optional<SimpleRxBufferEstimator> RxBufferEstimatorFromFidl(
    const netdriver::RxBufferManagement& management);

}  // namespace network::internal

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_SIMPLE_RX_BUFFER_ESTIMATOR_H_
