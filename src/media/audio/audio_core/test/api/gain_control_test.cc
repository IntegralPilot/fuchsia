// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/media/audio/audio_core/test/api/gain_control_test_shared.h"
#include "src/media/audio/lib/test/constants.h"

namespace media::audio::test {

TYPED_TEST(GainControlTest, SetGain) {
  constexpr float expect_gain_db = 20.0f;

  this->SetGain(expect_gain_db);
  this->ExpectGainCallback(expect_gain_db, false);

  this->SetGain(kUnityGainDb);
  this->ExpectGainCallback(kUnityGainDb, false);
}

TYPED_TEST(GainControlTest, SetMute) {
  bool expect_mute = true;

  this->SetMute(expect_mute);
  this->ExpectGainCallback(kUnityGainDb, expect_mute);

  expect_mute = false;
  this->SetMute(expect_mute);
  this->ExpectGainCallback(kUnityGainDb, expect_mute);
}

TYPED_TEST(GainControlTest, DuplicateSetGain) {
  constexpr float expect_gain_db = 20.0f;

  this->SetGain(expect_gain_db);
  this->ExpectGainCallback(expect_gain_db, false);

  this->SetGain(expect_gain_db);
  this->ExpectNoGainCallback();

  this->SetMute(true);
  this->ExpectGainCallback(expect_gain_db, true);
}

TYPED_TEST(GainControlTest, DuplicateSetMute) {
  constexpr float expect_gain_db = -42.0f;

  this->SetMute(true);
  this->ExpectGainCallback(kUnityGainDb, true);

  this->SetMute(true);
  this->ExpectNoGainCallback();

  this->SetGain(expect_gain_db);
  this->ExpectGainCallback(expect_gain_db, true);
}

// A gain-ramp with zero duration should take effect immediately.
TEST_F(GainControlRampTest, SetGainRampZeroDuration) {
  constexpr float expect_gain_db = -20.0f;
  this->SetGainWithRamp(expect_gain_db, zx::msec(0));

  this->ExpectGainCallback(expect_gain_db, false);
}

// A gain-ramp with negative duration should take effect immediately (without disconnect).
TEST_F(GainControlRampTest, SetGainRampNegativeDuration) {
  constexpr float expect_gain_db = 20.0f;
  this->SetGainWithRamp(expect_gain_db, zx::msec(-1));

  this->ExpectGainCallback(expect_gain_db, false);
}

}  // namespace media::audio::test
