// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/added-display-info.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <array>
#include <memory>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

namespace {

class AddedDisplayInfoTest : public ::testing::Test {
 private:
  fdf_testing::ScopedGlobalLogger logger_;
};

TEST_F(AddedDisplayInfoTest, CreateFromFidlTranscribesDisplayId) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr std::array<display::Mode, 1> kModes = {
      display::Mode({
          .active_width = 1024,
          .active_height = 768,
          .refresh_rate_millihertz = 60'000,
      }),
  };
  static constexpr std::array<display::PixelFormat, 1> kPixelFormats = {
      display::PixelFormat::kR8G8B8A8};

  std::array<fuchsia_hardware_display_types::wire::Mode, 1> fidl_modes = {kModes[0].ToFidl()};
  std::array<fuchsia_images2::wire::PixelFormat, 1> fidl_pixel_formats = {
      kPixelFormats[0].ToFidl()};

  const fuchsia_hardware_display_engine::wire::RawDisplayInfo fidl_display_info = {
      .display_id = kDisplayId.ToFidl(),
      .preferred_modes =
          fidl::VectorView<fuchsia_hardware_display_types::wire::Mode>::FromExternal(fidl_modes),
      .pixel_formats =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(fidl_pixel_formats),
  };

  zx::result<std::unique_ptr<AddedDisplayInfo>> added_display_info_result =
      AddedDisplayInfo::Create(fidl_display_info);
  ASSERT_OK(added_display_info_result);
  std::unique_ptr<AddedDisplayInfo> added_display_info =
      std::move(added_display_info_result).value();

  EXPECT_EQ(kDisplayId, added_display_info->display_id);
  EXPECT_THAT(added_display_info->preferred_modes, ::testing::SizeIs(1));
  EXPECT_THAT(added_display_info->pixel_formats, ::testing::SizeIs(1));
}

TEST_F(AddedDisplayInfoTest, CreateFromFidlTranscribesPixelFormats) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr std::array<display::Mode, 1> kModes = {
      display::Mode({
          .active_width = 1024,
          .active_height = 768,
          .refresh_rate_millihertz = 60'000,
      }),
  };
  static constexpr std::array<display::PixelFormat, 2> kPixelFormats = {
      display::PixelFormat::kR8G8B8A8, display::PixelFormat::kB8G8R8A8};

  std::array<fuchsia_hardware_display_types::wire::Mode, 1> fidl_modes = {kModes[0].ToFidl()};
  std::array<fuchsia_images2::wire::PixelFormat, 2> fidl_pixel_formats = {
      kPixelFormats[0].ToFidl(), kPixelFormats[1].ToFidl()};

  const fuchsia_hardware_display_engine::wire::RawDisplayInfo fidl_display_info = {
      .display_id = kDisplayId.ToFidl(),
      .preferred_modes =
          fidl::VectorView<fuchsia_hardware_display_types::wire::Mode>::FromExternal(fidl_modes),
      .pixel_formats =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(fidl_pixel_formats),
  };

  zx::result<std::unique_ptr<AddedDisplayInfo>> added_display_info_result =
      AddedDisplayInfo::Create(fidl_display_info);
  ASSERT_OK(added_display_info_result);
  std::unique_ptr<AddedDisplayInfo> added_display_info =
      std::move(added_display_info_result).value();

  EXPECT_THAT(added_display_info->pixel_formats, ::testing::ElementsAreArray(kPixelFormats));

  EXPECT_EQ(kDisplayId, added_display_info->display_id);
  EXPECT_THAT(added_display_info->preferred_modes, ::testing::SizeIs(1));
}

TEST_F(AddedDisplayInfoTest, CreateFromFidlCopiesDisplayModes) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr std::array<display::PixelFormat, 1> kPixelFormats = {
      display::PixelFormat::kR8G8B8A8};

  std::array<fuchsia_images2::wire::PixelFormat, 1> fidl_pixel_formats = {
      kPixelFormats[0].ToFidl()};
  std::array<fuchsia_hardware_display_types::wire::Mode, 2> fidl_display_modes = {{
      {
          .active_area = {.width = 640, .height = 480},
          .refresh_rate_millihertz = 60'000,
      },
      {
          .active_area = {.width = 1024, .height = 768},
          .refresh_rate_millihertz = 75'000,
      },
  }};

  const fuchsia_hardware_display_engine::wire::RawDisplayInfo fidl_display_info = {
      .display_id = kDisplayId.ToFidl(),
      .preferred_modes = fidl::VectorView<fuchsia_hardware_display_types::wire::Mode>::FromExternal(
          fidl_display_modes),
      .pixel_formats =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(fidl_pixel_formats),
  };

  zx::result<std::unique_ptr<AddedDisplayInfo>> added_display_info_result =
      AddedDisplayInfo::Create(fidl_display_info);
  ASSERT_OK(added_display_info_result);
  std::unique_ptr<AddedDisplayInfo> added_display_info =
      std::move(added_display_info_result).value();

  ASSERT_THAT(added_display_info->preferred_modes, ::testing::SizeIs(2));

  EXPECT_EQ(640, added_display_info->preferred_modes[0].active_area().width());
  EXPECT_EQ(480, added_display_info->preferred_modes[0].active_area().height());
  EXPECT_EQ(60'000, added_display_info->preferred_modes[0].refresh_rate_millihertz());

  EXPECT_EQ(1024, added_display_info->preferred_modes[1].active_area().width());
  EXPECT_EQ(768, added_display_info->preferred_modes[1].active_area().height());
  EXPECT_EQ(75'000, added_display_info->preferred_modes[1].refresh_rate_millihertz());

  EXPECT_EQ(kDisplayId, added_display_info->display_id);
  EXPECT_THAT(added_display_info->pixel_formats, ::testing::SizeIs(1));
}

}  // namespace

}  // namespace display_coordinator
