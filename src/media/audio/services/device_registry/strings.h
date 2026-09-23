// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_STRINGS_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_STRINGS_H_

#include <string_view>

namespace media_audio {

static inline constexpr const char *kAdrLoggingTag = "audio_device_registry";
static constexpr std::string_view kAdrSchedulerRole = "fuchsia.audio.device.registry.dispatch";
static inline constexpr const char *kAdrThreadName = "AudioDeviceRegistryMain";
static inline constexpr const char *kAdrTraceProvider = "audio_device_registry_provider";

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_STRINGS_H_
