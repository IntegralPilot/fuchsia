// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/signal_processing_utils.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/signal_processing_utils_unittest.h"

namespace media_audio {
namespace {

// These cases unittest the Map... functions with inputs that cause INFO logging (if any).

TEST(SignalProcessingUtilsTest, MapElements) {
  auto map = MapElements(kElements);
  EXPECT_EQ(map.size(), kElements.size());

  ASSERT_TRUE(map.contains(*kDaiInterconnectElement.id()));
  EXPECT_EQ(*map.at(*kDaiInterconnectElement.id()).element.type(), *kDaiInterconnectElement.type());
  EXPECT_TRUE(map.at(*kDaiInterconnectElement.id()).element.can_stop().value_or(false));

  ASSERT_TRUE(map.contains(*kRingBufferElement.id()));
  EXPECT_EQ(*map.at(*kRingBufferElement.id()).element.type(), *kRingBufferElement.type());
  ASSERT_TRUE(map.contains(*kPacketStreamElement.id()));
  EXPECT_EQ(*map.at(*kPacketStreamElement.id()).element.type(), *kPacketStreamElement.type());

  ASSERT_TRUE(map.contains(*kAgcElement.id()));
  EXPECT_EQ(*map.at(*kAgcElement.id()).element.type(), *kAgcElement.type());
  EXPECT_FALSE(map.at(*kAgcElement.id()).element.can_stop().value_or(true));
  EXPECT_EQ(map.at(*kAgcElement.id()).element.description()->at(255), 'X');

  ASSERT_TRUE(map.contains(*kDynamicsElement.id()));
  EXPECT_EQ(*map.at(*kDynamicsElement.id()).element.type(), *kDynamicsElement.type());
  EXPECT_TRUE(map.at(*kDynamicsElement.id()).element.can_bypass().value_or(false));
}

TEST(SignalProcessingUtilsTest, MapTopologies) {
  auto map = MapTopologies(kTopologies);
  EXPECT_EQ(map.size(), kTopologies.size());

  ASSERT_TRUE(map.contains(kTopologyDaiAgcDynRbId));
  EXPECT_EQ(map.at(kTopologyDaiAgcDynRbId).size(),
            3u);  // 3 edges: DAI -> AGC, AGC -> Dyn, Dyn -> RB
  EXPECT_EQ(map.at(kTopologyDaiAgcDynRbId).at(0).processing_element_id_from(),
            kDaiInterconnectElementId);
  EXPECT_EQ(map.at(kTopologyDaiAgcDynRbId).at(0).processing_element_id_to(), kAgcElementId);

  ASSERT_TRUE(map.contains(kTopologyDaiRbId));
  EXPECT_EQ(map.at(kTopologyDaiRbId).size(), 1u);  // 1 edge: DAI -> RB
  EXPECT_EQ(map.at(kTopologyDaiRbId).front().processing_element_id_from(),
            kDaiInterconnectElementId);
  EXPECT_EQ(map.at(kTopologyDaiRbId).front().processing_element_id_to(), kRingBufferElementId);

  ASSERT_TRUE(map.contains(kTopologyRbDaiId));
  EXPECT_EQ(map.at(kTopologyRbDaiId).size(), 1u);  // 1 edge: RB -> DAI
  EXPECT_EQ(map.at(kTopologyRbDaiId).front().processing_element_id_from(), kRingBufferElementId);
  EXPECT_EQ(map.at(kTopologyRbDaiId).front().processing_element_id_to(), kDaiInterconnectElementId);

  ASSERT_TRUE(map.contains(kTopologyPsDaiId));
  EXPECT_EQ(map.at(kTopologyPsDaiId).size(), 1u);  // 1 edge: PS -> DAI
  EXPECT_EQ(map.at(kTopologyPsDaiId).front().processing_element_id_from(), kPacketStreamElementId);
  EXPECT_EQ(map.at(kTopologyPsDaiId).front().processing_element_id_to(), kDaiInterconnectElementId);
}

TEST(SignalProcessingUtilsTest, ElementHasOutgoingEdges) {
  auto edge_pairs = *kTopologyDaiAgcDynRb.processing_elements_edge_pairs();
  EXPECT_TRUE(ElementHasOutgoingEdges(edge_pairs, kDaiInterconnectElementId));
  EXPECT_TRUE(ElementHasOutgoingEdges(edge_pairs, kAgcElementId));
  EXPECT_TRUE(ElementHasOutgoingEdges(edge_pairs, kDynamicsElementId));
  EXPECT_FALSE(ElementHasOutgoingEdges(edge_pairs, kRingBufferElementId));

  auto edge_pairs2 = *kTopologyPsDai.processing_elements_edge_pairs();
  EXPECT_TRUE(ElementHasOutgoingEdges(edge_pairs2, kPacketStreamElementId));
  EXPECT_FALSE(ElementHasOutgoingEdges(edge_pairs2, kDaiInterconnectElementId));
}

TEST(SignalProcessingUtilsTest, ElementHasIncomingEdges) {
  auto edge_pairs = *kTopologyDaiAgcDynRb.processing_elements_edge_pairs();
  EXPECT_FALSE(ElementHasIncomingEdges(edge_pairs, kDaiInterconnectElementId));
  EXPECT_TRUE(ElementHasIncomingEdges(edge_pairs, kAgcElementId));
  EXPECT_TRUE(ElementHasIncomingEdges(edge_pairs, kDynamicsElementId));
  EXPECT_TRUE(ElementHasIncomingEdges(edge_pairs, kRingBufferElementId));

  auto edge_pairs2 = *kTopologyPsDai.processing_elements_edge_pairs();
  EXPECT_FALSE(ElementHasIncomingEdges(edge_pairs2, kPacketStreamElementId));
  EXPECT_TRUE(ElementHasIncomingEdges(edge_pairs2, kDaiInterconnectElementId));
}

}  // namespace
}  // namespace media_audio
