// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/pipe.h"

#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/sysmem-version/sysmem-version.h>

#include <array>
#include <cstdint>
#include <map>
#include <optional>
#include <vector>

#include <gtest/gtest.h>

#include "src/graphics/display/drivers/intel-display/gtt.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace intel_display {

class PipeTest : public ::testing::Test {
 public:
  PipeTest() = default;

  void SetUp() override { mmio_buffer_.emplace(reg_region_.GetMmioBuffer()); }

  void TearDown() override {}

 protected:
  static constexpr uint32_t kMinimumRegCount = 0xd0000 / sizeof(uint32_t);
  fdf_testing::ScopedGlobalLogger logger_;
  fake_mmio::FakeMmioRegRegion reg_region_{sizeof(uint32_t), kMinimumRegCount};
  std::optional<fdf::MmioBuffer> mmio_buffer_;
};

namespace {

class TestGttRegionImpl : public GttRegion {
 public:
  explicit TestGttRegionImpl(uint64_t handle) : handle_(handle) {}

  uint64_t bytes_per_row() const override { return 64; }
  uint64_t base() const override { return handle_ + 0xf0000000; }

 private:
  uint64_t handle_ = 0;
};

std::map<display::DriverImageId, TestGttRegionImpl> region_map;

PixelFormatAndModifier GetPixelFormat(display::DriverImageId image_id) {
  return PixelFormatAndModifier(
      fuchsia_images2::PixelFormat::kB8G8R8A8,
      /*pixel_format_modifier_param=*/fuchsia_images2::PixelFormatModifier::kLinear);
}

const GttRegion& GetGttImageHandle(const display::ImageMetadata& image_metadata,
                                   display::DriverImageId image_handle,
                                   display::CoordinateTransformation coordinate_transformation) {
  auto it = region_map.find(image_handle);
  if (it != region_map.end()) {
    return it->second;
  }
  return region_map.try_emplace(image_handle, image_handle.value()).first->second;
}

display::DriverLayer CreatePrimaryLayerConfig(display::DriverImageId image_id) {
  static constexpr int32_t kWidth = 1024;
  static constexpr int32_t kHeight = 768;

  return display::DriverLayer({
      .display_destination = {{.x = 0, .y = 0, .width = kWidth, .height = kHeight}},
      .image_source = {{.x = 0, .y = 0, .width = kWidth, .height = kHeight}},
      .image_id = image_id,
      .image_metadata = {{.width = kWidth,
                          .height = kHeight,
                          .tiling_type = display::ImageTilingType::kLinear}},
      .fallback_color = {{
          .format = display::PixelFormat::kB8G8R8A8,
          .bytes = {{0xaa, 0xbb, 0xcc, 0xdd, 0, 0, 0, 0}},
      }},
      .alpha_mode = display::AlphaMode::kDisable,
      .alpha_coefficient = 0.0f,
      .image_source_transformation = display::CoordinateTransformation::kIdentity,
  });
}

}  // namespace

TEST_F(PipeTest, TiedTranscoderId) {
  PipeSkylake pipe_a(&mmio_buffer_.value(), PipeId::PIPE_A, {});
  EXPECT_EQ(TranscoderId::TRANSCODER_A, pipe_a.tied_transcoder_id());

  PipeSkylake pipe_b(&mmio_buffer_.value(), PipeId::PIPE_B, {});
  EXPECT_EQ(TranscoderId::TRANSCODER_B, pipe_b.tied_transcoder_id());

  PipeSkylake pipe_c(&mmio_buffer_.value(), PipeId::PIPE_C, {});
  EXPECT_EQ(TranscoderId::TRANSCODER_C, pipe_c.tied_transcoder_id());

  // TODO(https://fxbug.dev/42060657): Add a test for transcoder D, when we support it.
}

// Verifies that GetVsyncConfigStamp() could return the correct config stamp
// given different image handles from device registers.
TEST_F(PipeTest, GetVsyncConfigStamp) {
  PipeSkylake pipe(&*mmio_buffer_, PipeId::PIPE_A, {});

  display::DriverImageId kImageHandle1(0x1111u);
  display::DriverImageId kImageHandle2(0x2222u);
  display::DriverImageId kImageHandle3(0x3333u);
  display::DriverLayer layer_1 = CreatePrimaryLayerConfig(kImageHandle1);
  display::DriverLayer layer_2 = CreatePrimaryLayerConfig(kImageHandle2);
  display::DriverLayer layer_3 = CreatePrimaryLayerConfig(kImageHandle3);

  // Applies configuration with only one layer (layer_1).
  const std::array<display::DriverLayer, 1> test_layers1 = {layer_1};
  display::DriverConfigStamp stamp_1{1};
  pipe.SubmitConfiguration(display::ColorConversion::kIdentity, test_layers1, stamp_1,
                           GetGttImageHandle, GetPixelFormat);

  // For images that are not registered with Pipe yet, GetVsyncConfigStamp()
  // should return nullopt.
  display::DriverConfigStamp vsync_config_stamp_not_found =
      pipe.GetVsyncConfigStamp({kImageHandle2});
  EXPECT_EQ(vsync_config_stamp_not_found, display::kInvalidDriverConfigStamp);

  // Otherwise, for a valid image handle that has occurred in a past config,
  // GetVsyncConfigStamp() should return the latest config where it occurred.
  display::DriverConfigStamp vsync_config_stamp_1 = pipe.GetVsyncConfigStamp({kImageHandle1});
  EXPECT_NE(vsync_config_stamp_1, display::kInvalidDriverConfigStamp);
  EXPECT_EQ(vsync_config_stamp_1, stamp_1);

  // Applies another configuration with two layers (layer_2 replacing layer_1,
  // and a new layer layer_3).
  const std::array<display::DriverLayer, 2> test_layers2 = {layer_2, layer_3};
  display::DriverConfigStamp stamp_2{2};
  pipe.SubmitConfiguration(display::ColorConversion::kIdentity, test_layers2, stamp_2,
                           GetGttImageHandle, GetPixelFormat);

  // It is possible that a layer update is slower than other layers, so on
  // Vsync time the device may have layers from different configurations. In
  // that case, the device should return the oldest configuration stamp, i.e.
  // stamp_1.
  display::DriverConfigStamp vsync_config_stamp_2 =
      pipe.GetVsyncConfigStamp({kImageHandle1, kImageHandle3});
  EXPECT_NE(vsync_config_stamp_2, display::kInvalidDriverConfigStamp);
  EXPECT_EQ(vsync_config_stamp_2, stamp_1);

  // Now both layers are updated in another new Vsync. GetVsyncConfigStamp()
  // should return the updated stamp value.
  display::DriverConfigStamp vsync_config_stamp_3 =
      pipe.GetVsyncConfigStamp({kImageHandle2, kImageHandle3});
  EXPECT_NE(vsync_config_stamp_3, display::kInvalidDriverConfigStamp);
  EXPECT_EQ(vsync_config_stamp_3, stamp_2);

  // Old image handle should be evicted from Pipe completely.
  display::DriverConfigStamp vsync_config_stamp_4 =
      pipe.GetVsyncConfigStamp({kImageHandle1, kImageHandle3});
  EXPECT_EQ(vsync_config_stamp_4, display::kInvalidDriverConfigStamp);
}

}  // namespace intel_display
