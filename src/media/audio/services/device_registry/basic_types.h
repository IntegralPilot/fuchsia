// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_BASIC_TYPES_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_BASIC_TYPES_H_

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>

#include <array>

namespace media_audio {

// FIDL IDs.
using TokenId = fuchsia_audio_device::TokenId;
using ElementId = fuchsia_audio_device::ElementId;
using TopologyId = fuchsia_audio_device::TopologyId;
using ClockDomain = fuchsia_audio_device::ClockDomain;
using UniqueId = std::array<uint8_t, fuchsia_audio_device::kUniqueInstanceIdSize>;

// In addition to just mapping the ElementId to its unique Element, we also store the ElementState,
// as well as conveniently store what is needed to manage pending state-change notifications to
// clients. This enables us to interact properly with ObserverNotify instances.
struct ElementRecord {
  fuchsia_hardware_audio_signalprocessing::Element element;
  std::optional<fuchsia_hardware_audio_signalprocessing::ElementState> state;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_BASIC_TYPES_H_
