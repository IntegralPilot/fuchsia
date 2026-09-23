// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/engine-info.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include <gtest/gtest.h>

namespace display {

namespace {

constexpr EngineInfo kSingleLayerWithCapture({
    .max_layer_count = 1,
    .max_connected_display_count = 1,
    .is_capture_supported = true,
});

constexpr EngineInfo kSingleLayerWithCapture2({
    .max_layer_count = 1,
    .max_connected_display_count = 1,
    .is_capture_supported = true,
});

constexpr EngineInfo kMultiLayerWithoutCapture({
    .max_layer_count = 4,
    .max_connected_display_count = 1,
    .is_capture_supported = false,
});

TEST(EngineInfoTest, EqualityIsReflexive) {
  EXPECT_EQ(kSingleLayerWithCapture, kSingleLayerWithCapture);
  EXPECT_EQ(kSingleLayerWithCapture2, kSingleLayerWithCapture2);
  EXPECT_EQ(kMultiLayerWithoutCapture, kMultiLayerWithoutCapture);
}

TEST(EngineInfoTest, EqualityIsSymmetric) {
  EXPECT_EQ(kSingleLayerWithCapture, kSingleLayerWithCapture2);
  EXPECT_EQ(kSingleLayerWithCapture2, kSingleLayerWithCapture);
}

TEST(EngineInfoTest, EqualityForDifferentLayerCounts) {
  static constexpr EngineInfo kMultiLayerWithCapture({
      .max_layer_count = 4,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  });
  EXPECT_NE(kSingleLayerWithCapture, kMultiLayerWithCapture);
  EXPECT_NE(kMultiLayerWithCapture, kSingleLayerWithCapture);
}

TEST(EngineInfoTest, EqualityForDifferentCaptureSupport) {
  static constexpr EngineInfo kSingleLayerWithoutCapture({
      .max_layer_count = 1,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  });
  EXPECT_NE(kSingleLayerWithCapture, kSingleLayerWithoutCapture);
  EXPECT_NE(kSingleLayerWithoutCapture, kSingleLayerWithCapture);
}

TEST(EngineInfoTest, FromDesignatedInitializer) {
  static constexpr EngineInfo engine_info({
      .max_layer_count = 4,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  });
  EXPECT_EQ(4, engine_info.max_layer_count());
  EXPECT_EQ(1, engine_info.max_connected_display_count());
  EXPECT_EQ(false, engine_info.is_capture_supported());
}

TEST(EngineInfoTest, FromFidlEngineInfo) {
  static constexpr fuchsia_hardware_display_engine::wire::EngineInfo fidl_engine_info = {
      .max_layer_count = 4,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  };

  static constexpr EngineInfo engine_info = EngineInfo::From(fidl_engine_info);
  EXPECT_EQ(4, engine_info.max_layer_count());
  EXPECT_EQ(1, engine_info.max_connected_display_count());
  EXPECT_EQ(false, engine_info.is_capture_supported());
}

TEST(EngineInfoTest, ToFidlEngineInfo) {
  static constexpr EngineInfo engine_info({
      .max_layer_count = 4,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  });

  static constexpr fuchsia_hardware_display_engine::wire::EngineInfo fidl_engine_info =
      engine_info.ToFidl();
  EXPECT_EQ(4u, fidl_engine_info.max_layer_count);
  EXPECT_EQ(1u, fidl_engine_info.max_connected_display_count);
  EXPECT_EQ(false, fidl_engine_info.is_capture_supported);
}

TEST(EngineInfoTest, IsValidFidlMultiLayerCaptureSupported) {
  EXPECT_TRUE(EngineInfo::IsValid(fuchsia_hardware_display_engine::wire::EngineInfo{
      .max_layer_count = 4,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  }));
}

TEST(EngineInfoTest, IsValidFidlLargeMaxLayerCount) {
  EXPECT_FALSE(EngineInfo::IsValid(fuchsia_hardware_display_engine::wire::EngineInfo{
      .max_layer_count = 1'000,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  }));
}

TEST(EngineInfoTest, IsValidFidlZeroMaxLayerCount) {
  EXPECT_FALSE(EngineInfo::IsValid(fuchsia_hardware_display_engine::wire::EngineInfo{
      .max_layer_count = 0,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  }));
}

TEST(EngineInfoTest, IsValidFidlLargeMaxConnectedDisplayCount) {
  EXPECT_FALSE(EngineInfo::IsValid(fuchsia_hardware_display_engine::wire::EngineInfo{
      .max_layer_count = 1,
      .max_connected_display_count = 1'000,
      .is_capture_supported = false,
  }));
}

TEST(EngineInfoTest, IsValidFidlZeroMaxConnectedDisplayCount) {
  EXPECT_FALSE(EngineInfo::IsValid(fuchsia_hardware_display_engine::wire::EngineInfo{
      .max_layer_count = 1,
      .max_connected_display_count = 0,
      .is_capture_supported = false,
  }));
}

}  // namespace

}  // namespace display
