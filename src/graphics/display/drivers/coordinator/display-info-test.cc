// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/display-info.h"

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <memory>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/coordinator/added-display-info.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

namespace {

class DisplayInfoTest : public ::testing::Test {
 private:
  fdf_testing::ScopedGlobalLogger logger_;
};

TEST_F(DisplayInfoTest, InitializeWithPreferredModes) {
  static constexpr display::PixelFormat kPixelFormat = display::PixelFormat::kR8G8B8A8;
  const std::vector<display::Mode> kModes = {
      display::Mode({
          .active_width = 1024,
          .active_height = 768,
          .refresh_rate_millihertz = 60'000,
      }),
      display::Mode({
          .active_width = 800,
          .active_height = 600,
          .refresh_rate_millihertz = 75'000,
      }),
  };
  AddedDisplayInfo added_display_info = {
      .display_id = display::DisplayId(1),
      .pixel_formats =
          {
              kPixelFormat,
          },
      .preferred_modes = {kModes[0], kModes[1]},
  };
  zx::result<std::unique_ptr<DisplayInfo>> display_info_result =
      DisplayInfo::Create(std::move(added_display_info));
  ASSERT_OK(display_info_result);

  std::unique_ptr<DisplayInfo> display_info = std::move(display_info_result).value();
  EXPECT_THAT(display_info->pixel_formats, ::testing::ElementsAre(kPixelFormat));
  EXPECT_THAT(display_info->preferred_modes,
              ::testing::ElementsAre(display::ModeAndId({display::ModeId(1), kModes[0]}),
                                     display::ModeAndId({display::ModeId(2), kModes[1]})));
}

TEST_F(DisplayInfoTest, InitializeFailureOnEmptyPreferredModes) {
  AddedDisplayInfo added_display_info = {
      .display_id = display::DisplayId(1),
      .pixel_formats =
          {
              display::PixelFormat::kR8G8B8A8,
          },
      .preferred_modes = {},
  };
  zx::result<std::unique_ptr<DisplayInfo>> display_info_result =
      DisplayInfo::Create(std::move(added_display_info));
  EXPECT_STATUS(display_info_result, ZX_ERR_INVALID_ARGS);
}

}  // namespace

}  // namespace display_coordinator
