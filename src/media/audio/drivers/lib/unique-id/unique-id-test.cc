// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/lib/unique-id/unique-id.h"

#include <gtest/gtest.h>

namespace audio {
namespace {

TEST(UniqueIdTest, SingularUniqueIdTest) {
  auto verify_unique_id = [](fuchsia_hardware_audio::CompositeProperties properties,
                             fuchsia_hardware_audio::SingularUniqueId singular_id) {
    ASSERT_TRUE(properties.unique_id().has_value());
    const std::array<uint8_t, fuchsia_hardware_audio::kUniqueIdSize>& unique_id =
        properties.unique_id().value();

    uint32_t i = 0;
    EXPECT_EQ(unique_id[i++], static_cast<uint8_t>(singular_id));
    while (i < fuchsia_hardware_audio::kUniqueIdSize) {
      EXPECT_EQ(unique_id[i++], 0);
    }
  };

  verify_unique_id(audio::DspProperties(), fuchsia_hardware_audio::SingularUniqueId::kDsp);
  verify_unique_id(audio::BuiltinSpeakerProperties(),
                   fuchsia_hardware_audio::SingularUniqueId::kBuiltinSpeaker);
  verify_unique_id(audio::BuiltinHeadphoneJackProperties(),
                   fuchsia_hardware_audio::SingularUniqueId::kBuiltinHeadphoneJack);
  verify_unique_id(audio::BuiltinMicrophoneProperties(),
                   fuchsia_hardware_audio::SingularUniqueId::kBuiltinMicrophone);
  verify_unique_id(audio::BuiltinHeadsetJackProperties(),
                   fuchsia_hardware_audio::SingularUniqueId::kBuiltinHeadsetJack);
}

}  // namespace
}  // namespace audio
