// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_LIB_UNIQUE_ID_UNIQUE_ID_H_
#define SRC_MEDIA_AUDIO_DRIVERS_LIB_UNIQUE_ID_UNIQUE_ID_H_

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>

namespace audio {

namespace {

fuchsia_hardware_audio::CompositeProperties PropertiesWithSingularId(
    fuchsia_hardware_audio::SingularUniqueId singular_id) {
  return fuchsia_hardware_audio::CompositeProperties({
      .unique_id = std::array<uint8_t, fuchsia_hardware_audio::kUniqueIdSize>{static_cast<uint8_t>(
          singular_id)},
  });
}

}  // namespace

fuchsia_hardware_audio::CompositeProperties DspProperties() {
  return PropertiesWithSingularId(fuchsia_hardware_audio::SingularUniqueId::kDsp);
}

fuchsia_hardware_audio::CompositeProperties BuiltinSpeakerProperties() {
  return PropertiesWithSingularId(fuchsia_hardware_audio::SingularUniqueId::kBuiltinSpeaker);
}

fuchsia_hardware_audio::CompositeProperties BuiltinHeadphoneJackProperties() {
  return PropertiesWithSingularId(fuchsia_hardware_audio::SingularUniqueId::kBuiltinHeadphoneJack);
}

fuchsia_hardware_audio::CompositeProperties BuiltinMicrophoneProperties() {
  return PropertiesWithSingularId(fuchsia_hardware_audio::SingularUniqueId::kBuiltinMicrophone);
}

fuchsia_hardware_audio::CompositeProperties BuiltinHeadsetJackProperties() {
  return PropertiesWithSingularId(fuchsia_hardware_audio::SingularUniqueId::kBuiltinHeadsetJack);
}

}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_LIB_UNIQUE_ID_UNIQUE_ID_H_
