// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/media/cpp/fidl.h>

#include <cmath>

#include <gtest/gtest.h>

#include "src/media/audio/audio_core/test/api/gain_control_test_shared.h"
#include "src/media/audio/lib/test/constants.h"

namespace media::audio::test {

template <typename RendererOrCapturerT>
class GainControlErrorTest : public GainControlTest<RendererOrCapturerT> {};
TYPED_TEST_SUITE(GainControlErrorTest, GainControlTestTypes, GainControlTypeNames);

// Setting gain too high should cause a disconnect.
TYPED_TEST(GainControlErrorTest, SetGainTooHigh) {
  this->SetGain(kTooHighGainDb);

  this->ExpectParentDisconnect();
  EXPECT_FALSE(this->gain_control_1().is_bound());
  EXPECT_FALSE(this->gain_control_2().is_bound());
}

// Setting gain too low should cause a disconnect.
TYPED_TEST(GainControlErrorTest, SetGainTooLow) {
  this->SetGain(kTooLowGainDb);

  this->ExpectParentDisconnect();
  EXPECT_FALSE(this->gain_control_1().is_bound());
  EXPECT_FALSE(this->gain_control_2().is_bound());
}

// Setting gain to NAN should cause a disconnect.
TYPED_TEST(GainControlErrorTest, SetGainNaN) {
  this->SetGain(NAN);

  this->ExpectParentDisconnect();
  EXPECT_FALSE(this->gain_control_1().is_bound());
  EXPECT_FALSE(this->gain_control_2().is_bound());
}

// SetGainWithRamp is not implemented for GainControl interfaces created from AudioCapturer
class GainControlRampErrorTest : public GainControlErrorTest<fuchsia::media::AudioRendererPtr> {};

// Setting ramp target-gain too high should cause a disconnect of the parent and gain interfaces.
TEST_F(GainControlRampErrorTest, SetGainRampTooHigh) {
  this->SetGainWithRamp(kTooHighGainDb, zx::msec(1));

  this->ExpectParentDisconnect();
  EXPECT_FALSE(this->gain_control_1().is_bound());
  EXPECT_FALSE(this->gain_control_2().is_bound());
}

// Setting ramp target-gain too low should cause a disconnect of the parent and gain interfaces.
TEST_F(GainControlRampErrorTest, SetGainRampTooLow) {
  this->SetGainWithRamp(kTooLowGainDb, zx::msec(1));

  this->ExpectParentDisconnect();
  EXPECT_FALSE(this->gain_control_1().is_bound());
  EXPECT_FALSE(this->gain_control_2().is_bound());
}

// Setting a gain-ramp with NaN target gain should cause parent and children to disconnect.
TEST_F(GainControlRampErrorTest, SetGainRampNaN) {
  this->SetGainWithRamp(NAN, zx::msec(1));

  this->ExpectParentDisconnect();
  EXPECT_FALSE(this->gain_control_1().is_bound());
  EXPECT_FALSE(this->gain_control_2().is_bound());
}

}  // namespace media::audio::test
