// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_TESTS_BASIC_TEST_H_
#define SRC_MEDIA_AUDIO_DRIVERS_TESTS_BASIC_TEST_H_

#include <fuchsia/hardware/audio/cpp/fidl.h>
#include <zircon/device/audio.h>

#include <optional>

#include "src/media/audio/drivers/tests/test_base.h"

namespace media::audio::drivers::test {

// BasicTest cases must run in environments where an audio driver may already have an active client.
// For most hardware functions, testing is limited to validating the correctness and consistency of
// the declared capabilities and current state. BasicTest cases CAN _change_ a device's state, but
// only if it fully restores the previous state afterward (as it does when testing SetGain).
//
// A driver can have only one RingBuffer client connection at any time, so BasicTest avoids any
// usage of the RingBuffer interface. (Note: AdminTest is not limited to RingBuffer cases.)
class BasicTest : public TestBase {
 public:
  explicit BasicTest(const DeviceEntry& dev_entry) : TestBase(dev_entry) {}

 protected:
  void TearDown() override;

  void WatchGainStateAndExpectUpdate();
  void WatchGainStateAndExpectNoUpdate();

  void SetGainStateChange();
  void SetGainStateNoChange();
  void SetGainStateNoValues();
  void SetImpossibleGainDb(float gain_db);
  void SetImpossibleMute();
  void SetImpossibleAgc();

  void WatchPlugStateAndExpectUpdate();
  void WatchPlugStateAndExpectNoUpdate();

  float min_gain_db() const { return *(properties()->min_gain_db); }
  float max_gain_db() const { return *(properties()->max_gain_db); }

 private:
  void RequestSetGain(fuchsia::hardware::audio::GainState gain_state);
  void ValidatePlugState(const fuchsia::hardware::audio::PlugState& plug_state);

  // BasicTest cannot permanently change device state. Optionals ensure we fetch initial gain
  // state (to later restore it), before calling any method that alters device gain.
  std::optional<fuchsia::hardware::audio::GainState> initial_gain_state_;
  // If this is set, it means that we have changed the preexisting gain state of the device.
  std::optional<fuchsia::hardware::audio::GainState> expected_gain_state_;
};

}  // namespace media::audio::drivers::test

#endif  // SRC_MEDIA_AUDIO_DRIVERS_TESTS_BASIC_TEST_H_
