// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/signal_processing_utils.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/natural_types.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>

namespace media_audio {

namespace fhasp = fuchsia_hardware_audio_signalprocessing;

std::unordered_set<ElementId> dais(
    const std::unordered_map<ElementId, ElementRecord>& element_map) {
  std::unordered_set<ElementId> dais;
  for (const auto& element_entry_pair : element_map) {
    if (element_entry_pair.second.element.type() == fhasp::ElementType::kDaiInterconnect) {
      dais.insert(element_entry_pair.first);
    }
  }
  return dais;
}

std::unordered_set<ElementId> ring_buffers(
    const std::unordered_map<ElementId, ElementRecord>& element_map) {
  std::unordered_set<ElementId> ring_buffers;
  for (const auto& element_entry_pair : element_map) {
    if (element_entry_pair.second.element.type() == fhasp::ElementType::kRingBuffer) {
      ring_buffers.insert(element_entry_pair.first);
    }
  }
  return ring_buffers;
}

std::unordered_set<ElementId> packet_streams(
    const std::unordered_map<ElementId, ElementRecord>& element_map) {
  std::unordered_set<ElementId> packet_streams;
  for (const auto& element_entry_pair : element_map) {
    if (element_entry_pair.second.element.type() == fhasp::ElementType::kPacketStream) {
      packet_streams.insert(element_entry_pair.first);
    }
  }
  return packet_streams;
}

// This maps ElementId->ElementRecord but populates only the Element portion of the ElementRecord.
std::unordered_map<ElementId, ElementRecord> MapElements(
    const std::vector<fhasp::Element>& elements) {
  auto element_map = std::unordered_map<ElementId, ElementRecord>{};

  for (const auto& element : elements) {
    if (!element.id().has_value()) {
      FX_LOGS(WARNING) << "invalid element_id";
      return {};
    }
    auto element_insertion = element_map.insert({*element.id(), ElementRecord{
                                                                    .element = element,
                                                                }});
    if (!element_insertion.second) {
      FX_LOGS(WARNING) << "duplicate element_id " << *element.id();
      return {};
    }
  }
  return element_map;
}

// Returns empty map if any topology_id values are duplicated.
std::unordered_map<TopologyId, std::vector<fhasp::EdgePair>> MapTopologies(
    const std::vector<fhasp::Topology>& topologies) {
  auto topology_map = std::unordered_map<TopologyId, std::vector<fhasp::EdgePair>>{};

  for (const auto& topology : topologies) {
    if (!topology.id().has_value() || !topology.processing_elements_edge_pairs().has_value() ||
        topology.processing_elements_edge_pairs()->empty()) {
      FX_LOGS(WARNING) << "incomplete topology";
      return {};
    }
    if (!topology_map.insert({*topology.id(), *topology.processing_elements_edge_pairs()}).second) {
      FX_LOGS(WARNING) << "Cannot map duplicate topology_id " << *topology.id();
      return {};
    }
  }
  return topology_map;
}

bool ElementHasOutgoingEdges(const std::vector<fhasp::EdgePair>& topology, ElementId element_id) {
  return std::any_of(topology.begin(), topology.end(), [element_id](const fhasp::EdgePair& pair) {
    return (pair.processing_element_id_from() == element_id);
  });
}

bool ElementHasIncomingEdges(const std::vector<fhasp::EdgePair>& topology, ElementId element_id) {
  return std::any_of(topology.begin(), topology.end(), [element_id](const fhasp::EdgePair& pair) {
    return (pair.processing_element_id_to() == element_id);
  });
}

}  // namespace media_audio
