// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "simple_rx_buffer_estimator.h"

#include <lib/zx/port.h>
#include <lib/zx/time.h>

#include <gtest/gtest.h>

namespace network {
namespace testing {

TEST(SimpleRxBufferEstimatorTest, CalculationAndBurst) {
  // Create estimator with alpha = 0.5 and delay budget = 5ms
  internal::SimpleRxBufferEstimator estimator(0.5, zx::msec(5));

  // Initial target buffers should be 0.
  EXPECT_EQ(estimator.CalculateTargetBuffers(), 0);

  // Update with 10,000 pps.
  // EWMA with initial 0: pps_scaled = (0.5 * 10000 + 0.5 * 0) = 5000 pps.
  // Target = 5000 * 0.005s = 25 buffers.
  estimator.Update(10'000);
  EXPECT_EQ(estimator.CalculateTargetBuffers(), 25);

  // Update with 10,000 pps again.
  // EWMA: pps_scaled = (0.5 * 10000 + 0.5 * 5000) = 7500 pps.
  // Target = 7500 * 0.005s = 37 buffers.
  estimator.Update(10'000);
  EXPECT_EQ(estimator.CalculateTargetBuffers(), 37);

  // A large burst (e.g. 100,000 pps) should trigger immediate buffers.
  uint16_t burst_target = estimator.NeedImmediateBuffers(100'000);
  EXPECT_GT(burst_target, 0);
  EXPECT_EQ(burst_target, 500);
  EXPECT_EQ(estimator.CalculateTargetBuffers(), 500);

  // Rate increase below threshold should return 0 immediate buffers.
  EXPECT_EQ(estimator.NeedImmediateBuffers(500), 0);
}

TEST(SimpleRxBufferEstimatorTest, CalculateTargetBuffersOverflow) {
  internal::SimpleRxBufferEstimator estimator(1.0, zx::sec(1));
  // Massive pps that overflows uint16_t (e.g. 100,000 pps for 1 sec = 100,000 > 65535).
  estimator.Update(100'000);
  EXPECT_EQ(estimator.CalculateTargetBuffers(), std::numeric_limits<uint16_t>::max());
}

TEST(SimpleRxBufferEstimatorTest, EstimatorFromFidl) {
  netdriver::Simple simple_defaults;
  netdriver::RxBufferManagement default_params =
      netdriver::RxBufferManagement::WithSimple(simple_defaults);
  std::optional default_estimator = internal::RxBufferEstimatorFromFidl(default_params);
  ASSERT_TRUE(default_estimator.has_value());
  EXPECT_NEAR(default_estimator->alpha(), 0.2, 0.005);
  EXPECT_EQ(default_estimator->delay_budget(), zx::msec(1));
  EXPECT_EQ(default_estimator->sample_interval(), zx::sec(1));
  EXPECT_EQ(default_estimator->variance_threshold(), 2.0);

  netdriver::Simple simple;
  simple.alpha(0.25);
  simple.delay_budget(ZX_MSEC(10));
  simple.sample_interval(ZX_MSEC(500));
  simple.variance_threshold(3.0);
  netdriver::RxBufferManagement simple_params = netdriver::RxBufferManagement::WithSimple(simple);

  std::optional estimator = internal::RxBufferEstimatorFromFidl(simple_params);
  ASSERT_TRUE(estimator.has_value());
  EXPECT_EQ(estimator->alpha(), 0.25);
  EXPECT_EQ(estimator->delay_budget(), zx::msec(10));
  EXPECT_EQ(estimator->sample_interval(), zx::msec(500));
  EXPECT_EQ(estimator->variance_threshold(), 3.0);

  netdriver::RxBufferManagement static_params =
      netdriver::RxBufferManagement::WithStatic_(netdriver::Static{});
  std::optional static_estimator = internal::RxBufferEstimatorFromFidl(static_params);
  EXPECT_FALSE(static_estimator.has_value());
}

TEST(SimpleRxBufferEstimatorTest, NeedImmediateBuffersTriggerAndReset) {
  internal::SimpleRxBufferEstimator estimator(0.5, zx::msec(5), zx::sec(1), 2.0);
  estimator.Update(10'000);
  EXPECT_EQ(estimator.CalculateTargetBuffers(), 25);

  uint16_t immediate = estimator.NeedImmediateBuffers(100'000);
  EXPECT_EQ(immediate, 500);

  EXPECT_EQ(estimator.NeedImmediateBuffers(100'000), 0);
}

TEST(SimpleRxBufferEstimatorTest, NeedImmediateBuffersThresholdSensitivity) {
  internal::SimpleRxBufferEstimator sensitive(1.0, zx::msec(5), zx::sec(1), 1.0);
  internal::SimpleRxBufferEstimator tolerant(1.0, zx::msec(5), zx::sec(1), 5.0);

  sensitive.Update(10'000);
  tolerant.Update(10'000);

  EXPECT_GT(sensitive.NeedImmediateBuffers(25'000), 0);
  EXPECT_EQ(tolerant.NeedImmediateBuffers(25'000), 0);
}

TEST(SimpleRxBufferEstimatorTest, TimerRearmAndCancel) {
  zx::port port;
  ASSERT_EQ(zx::port::create(0, &port), ZX_OK);

  internal::SimpleRxBufferEstimator estimator(0.5, zx::msec(5), zx::msec(5), 2.0);
  constexpr uint64_t kTestKey = 1234;

  EXPECT_EQ(estimator.RearmTimer(port, kTestKey), ZX_OK);

  zx_port_packet_t packet{};
  EXPECT_EQ(port.wait(zx::time::infinite(), &packet), ZX_OK);
  EXPECT_EQ(packet.key, kTestKey);
  EXPECT_EQ(packet.type, ZX_PKT_TYPE_SIGNAL_ONE);

  // Rearm and cancel should remove timer / cancel port wait.
  EXPECT_EQ(estimator.RearmTimer(port, kTestKey), ZX_OK);
  estimator.CancelTimer(port, kTestKey);
}

}  // namespace testing
}  // namespace network
