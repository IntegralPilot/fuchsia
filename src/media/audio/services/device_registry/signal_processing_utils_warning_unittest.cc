// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/signal_processing_utils.h"
#include "src/media/audio/services/device_registry/signal_processing_utils_unittest.h"

namespace media_audio {
namespace {

// These cases unittest the Map... functions with inputs that cause WARNING logging.

TEST(SignalProcessingUtilsWarningTest, BadElementList) {
  EXPECT_TRUE(MapElements(kEmptyElements).empty());

  // List contains two elements with the same id.
  EXPECT_TRUE(MapElements(kElementsDuplicateId).empty());

  // bad Element: missing id.
  EXPECT_TRUE(MapElements(kElementsWithNoId).empty());
}

TEST(SignalProcessingUtilsWarningTest, BadTopologyList) {
  EXPECT_TRUE(MapTopologies(kEmptyTopologies).empty());

  // List contains two topologies with the same id.
  EXPECT_TRUE(MapTopologies(kTopologiesWithDuplicateId).empty());

  // Topology list with a bad Topology: all the ValidateTopology negative cases.
  EXPECT_TRUE(MapTopologies(kTopologiesWithMissingId).empty());
  EXPECT_TRUE(MapTopologies(kTopologiesWithMissingEdgePairs).empty());
  EXPECT_TRUE(MapTopologies(kTopologiesWithEmptyEdgePairs).empty());
}

}  // namespace
}  // namespace media_audio
