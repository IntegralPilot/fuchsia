// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_SIGNAL_PROCESSING_UTILS_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_SIGNAL_PROCESSING_UTILS_H_

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>

#include <unordered_map>
#include <unordered_set>

#include "src/media/audio/services/device_registry/basic_types.h"

namespace media_audio {

std::unordered_map<ElementId, ElementRecord> MapElements(
    const std::vector<fuchsia_hardware_audio_signalprocessing::Element>& elements);
std::unordered_set<ElementId> dais(const std::unordered_map<ElementId, ElementRecord>& element_map);
std::unordered_set<ElementId> ring_buffers(
    const std::unordered_map<ElementId, ElementRecord>& element_map);
std::unordered_set<ElementId> packet_streams(
    const std::unordered_map<ElementId, ElementRecord>& element_map);

std::unordered_map<TopologyId, std::vector<fuchsia_hardware_audio_signalprocessing::EdgePair>>
MapTopologies(const std::vector<fuchsia_hardware_audio_signalprocessing::Topology>& topologies);

bool ElementHasOutgoingEdges(
    const std::vector<fuchsia_hardware_audio_signalprocessing::EdgePair>& topology,
    ElementId element_id);
bool ElementHasIncomingEdges(
    const std::vector<fuchsia_hardware_audio_signalprocessing::EdgePair>& topology,
    ElementId element_id);

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_SIGNAL_PROCESSING_UTILS_H_
