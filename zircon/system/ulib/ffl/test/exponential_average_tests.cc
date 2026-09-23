// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdint>

#include <ffl/exponential_average.h>
#include <zxtest/zxtest.h>

using ffl::ExponentialAverage;
using ffl::Fixed;
using ffl::FromInteger;
using ffl::FromRatio;

TEST(FuchsiaFixedPoint, ExponentialAverageSingleRate) {
  Fixed<zx_duration_t, 0> initial_value = FromInteger(ZX_MSEC(1));
  const Fixed<int32_t, 2> alpha = FromRatio(1, 2);
  ExponentialAverage avg(initial_value, alpha);
  EXPECT_EQ(avg.value().raw_value(), initial_value.raw_value());

  struct TestCase {
    Fixed<zx_duration_t, 0> sample;
    Fixed<zx_duration_t, 0> expected_avg;
    Fixed<zx_duration_t, 0> expected_variance;
  };
  struct TestCase test_cases[] = {
      {FromInteger(ZX_MSEC(1)), FromInteger(ZX_MSEC(1)), FromInteger(0)},
      {FromInteger(ZX_MSEC(2)), FromInteger(1500000), FromInteger(250000000000)},
      {FromInteger(ZX_MSEC(5)), FromInteger(3250000), FromInteger(3187500000000)},
      {FromInteger(ZX_MSEC(3)), FromInteger(3125000), FromInteger(1609375000000)},
      {FromInteger(ZX_MSEC(1)), FromInteger(2062500), FromInteger(1933593750000)},
  };

  for (auto test_case : test_cases) {
    avg.AddSample(test_case.sample);
    EXPECT_EQ(avg.value().raw_value(), test_case.expected_avg);
    EXPECT_EQ(avg.variance().raw_value(), test_case.expected_variance);
  }
}

TEST(FuchsiaFixedPoint, ExponentialAverageDualRate) {
  const Fixed<int32_t, 2> alpha = FromRatio(1, 4);
  const Fixed<int32_t, 2> beta = FromRatio(1, 2);
  Fixed<zx_duration_t, 0> initial_value = FromInteger(ZX_MSEC(1));
  ExponentialAverage avg(initial_value, alpha, beta);
  EXPECT_EQ(avg.value().raw_value(), initial_value.raw_value());

  struct TestCase {
    Fixed<zx_duration_t, 0> sample;
    Fixed<zx_duration_t, 0> expected_avg;
    Fixed<zx_duration_t, 0> expected_variance;
  };
  struct TestCase test_cases[] = {
      {FromInteger(ZX_MSEC(2)), FromInteger(1500000), FromInteger(250000000000)},
      {FromInteger(ZX_MSEC(5)), FromInteger(3250000), FromInteger(3187500000000)},
      {FromInteger(ZX_MSEC(1)), FromInteger(2687500), FromInteger(3339843750000)},
      {FromInteger(ZX_MSEC(1)), FromInteger(2265625), FromInteger(3038818359375)},
  };

  for (auto test_case : test_cases) {
    avg.AddSample(test_case.sample);
    EXPECT_EQ(avg.value().raw_value(), test_case.expected_avg);
    EXPECT_EQ(avg.variance().raw_value(), test_case.expected_variance);
  }
}
