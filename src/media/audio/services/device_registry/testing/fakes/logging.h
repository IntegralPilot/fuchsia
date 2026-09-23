// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_LOGGING_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_LOGGING_H_

#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

inline constexpr bool kLogFakeCodec = false;
inline constexpr bool kLogFakeComposite = false;
inline constexpr bool kLogFakeCompositeSignalProcessing = false;
inline constexpr bool kLogFakeCompositePacketStream = false;
inline constexpr bool kLogFakeCompositeRingBuffer = false;

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_LOGGING_H_
