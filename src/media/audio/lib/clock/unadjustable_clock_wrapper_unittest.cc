// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/lib/clock/unadjustable_clock_wrapper.h"

#include <lib/zx/time.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/media/audio/lib/clock/real_clock.h"
#include "src/media/audio/lib/clock/synthetic_clock.h"
#include "src/media/audio/lib/clock/synthetic_clock_realm.h"

namespace media_audio {
namespace {

zx::clock NewRealClock(zx_rights_t rights = ZX_RIGHT_DUPLICATE | ZX_RIGHT_READ | ZX_RIGHT_WRITE) {
  zx::clock clock;
  auto status = zx::clock::create(
      ZX_CLOCK_OPT_AUTO_START | ZX_CLOCK_OPT_MONOTONIC | ZX_CLOCK_OPT_CONTINUOUS, nullptr, &clock);
  FX_CHECK(status == ZX_OK) << "clock.create failed, status is " << status;

  status = clock.replace(rights, &clock);
  FX_CHECK(status == ZX_OK) << "clock.replace failed, status is " << status;

  return clock;
}

TEST(UnadjustableClockWrapperTest, FromRealAdjustable) {
  const zx_rights_t rights = ZX_RIGHT_DUPLICATE | ZX_RIGHT_READ | ZX_RIGHT_WRITE;
  auto adjustable_clock =
      RealClock::Create("real adjustable", NewRealClock(rights), Clock::kExternalDomain, true);
  auto unadjustable_wrapper = std::make_unique<UnadjustableClockWrapper>(adjustable_clock);

  EXPECT_EQ(unadjustable_wrapper->name(), adjustable_clock->name());
  EXPECT_EQ(unadjustable_wrapper->koid(), adjustable_clock->koid());
  EXPECT_EQ(unadjustable_wrapper->domain(), adjustable_clock->domain());
  EXPECT_EQ(unadjustable_wrapper->to_clock_mono(), adjustable_clock->to_clock_mono());

  EXPECT_TRUE(adjustable_clock->adjustable());
  EXPECT_FALSE(unadjustable_wrapper->adjustable());
  EXPECT_TRUE(unadjustable_wrapper->IdenticalToMonotonicClock());

  adjustable_clock->SetRate(42);
  EXPECT_EQ(unadjustable_wrapper->rate_adjustment_ppm(), 42);
  EXPECT_EQ(unadjustable_wrapper->to_clock_mono().rate(), adjustable_clock->to_clock_mono().rate());
  EXPECT_FALSE(unadjustable_wrapper->IdenticalToMonotonicClock());
}

TEST(UnadjustableClockWrapperTest, FromSynthetic) {
  auto realm = SyntheticClockRealm::Create();
  realm->AdvanceTo(zx::time(100));
  auto synthetic_clock = realm->CreateClock("synthetic adjustable", Clock::kExternalDomain, true);
  auto unadjustable_wrapper = std::make_unique<UnadjustableClockWrapper>(synthetic_clock);

  EXPECT_EQ(unadjustable_wrapper->name(), synthetic_clock->name());
  EXPECT_EQ(unadjustable_wrapper->koid(), synthetic_clock->koid());
  EXPECT_EQ(unadjustable_wrapper->domain(), synthetic_clock->domain());
  EXPECT_EQ(unadjustable_wrapper->to_clock_mono(), synthetic_clock->to_clock_mono());
  EXPECT_EQ(unadjustable_wrapper->now(), synthetic_clock->now());

  EXPECT_TRUE(synthetic_clock->adjustable());
  EXPECT_FALSE(unadjustable_wrapper->adjustable());
  EXPECT_TRUE(unadjustable_wrapper->IdenticalToMonotonicClock());

  synthetic_clock->SetRate(-42);
  EXPECT_EQ(unadjustable_wrapper->rate_adjustment_ppm(), -42);
  EXPECT_EQ(unadjustable_wrapper->to_clock_mono().rate(), synthetic_clock->to_clock_mono().rate());
  EXPECT_FALSE(unadjustable_wrapper->IdenticalToMonotonicClock());
}

}  // namespace
}  // namespace media_audio
