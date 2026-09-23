// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <memory>
#include <vector>

#include <gtest/gtest.h>
#include <test/accessibility/cpp/fidl.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/ui/testing/ui_test_manager/ui_test_manager.h"
#include "src/ui/testing/ui_test_realm/ui_test_realm.h"
#include "src/ui/testing/util/test_view.h"

namespace integration_tests {
namespace {

constexpr auto kViewProvider = "view-provider";
constexpr float kEpsilon = 0.005f;

struct DisplayConfig {
  uint32_t width_px;
  uint32_t height_px;
  uint32_t refresh_rate_millihertz;
  uint32_t max_layer_count;
};

constexpr DisplayConfig kSherlockDisplayConfig = {
    .width_px = 1280,
    .height_px = 800,
    .refresh_rate_millihertz = 60'000,
    .max_layer_count = 1,
};

}  // namespace

using component_testing::ChildRef;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::Realm;
using component_testing::Route;

// This test verifies that Scene Manager propagates
// 'config/data/device_pixel_ratio' correctly.
class DisplayPixelRatioTest
    : public gtest::RealLoopFixture,
      public ::testing::WithParamInterface<std::tuple</* pixel_ratio */ float, DisplayConfig>> {
 protected:
  // |testing::Test|
  void SetUp() override {
    const auto& [device_pixel_ratio, display_config] = GetParam();

    ui_testing::UITestRealm::Config config;
    config.use_scene_owner = true;
    config.accessibility_owner = ui_testing::UITestRealm::AccessibilityOwnerType::FAKE;
    config.ui_to_client_services.push_back(fuchsia::ui::composition::FlatlandFactory::Name_);
    config.device_pixel_ratio = device_pixel_ratio;
    config.display_config = {
        .active_width_px = display_config.width_px,
        .active_height_px = display_config.height_px,
        .refresh_rate_millihertz = display_config.refresh_rate_millihertz,
        .max_layer_count = display_config.max_layer_count,
    };
    ui_test_manager_.emplace(config);

    // Build realm.
    FX_LOGS(INFO) << "Building realm";
    realm_ = ui_test_manager_->AddSubrealm();

    // Add a test view provider.
    test_view_access_ = std::make_shared<ui_testing::TestViewAccess>();
    component_testing::LocalComponentFactory test_view = [d = dispatcher(),
                                                          a = test_view_access_]() {
      return std::make_unique<ui_testing::TestView>(
          d, /* content = */ ui_testing::TestView::ContentType::COORDINATE_GRID, a);
    };

    realm_->AddLocalChild(kViewProvider, std::move(test_view));
    realm_->AddRoute(Route{.capabilities = {Protocol{fuchsia::ui::app::ViewProvider::Name_}},
                           .source = ChildRef{kViewProvider},
                           .targets = {ParentRef()}});

    for (const auto& protocol : config.ui_to_client_services) {
      realm_->AddRoute(Route{.capabilities = {Protocol{protocol}},
                             .source = ParentRef(),
                             .targets = {ChildRef{kViewProvider}}});
    }

    ui_test_manager_->BuildRealm();
    realm_exposed_services_ = ui_test_manager_->CloneExposedServicesDirectory();

    // Attach view, and wait for it to render.
    ui_test_manager_->InitializeScene();
    RunLoopUntil([this]() { return ui_test_manager_->ClientViewIsRendering(); });

    // Get display's width and height.
    auto [width, height] = ui_test_manager_->GetDisplayDimensions();

    display_width_ = static_cast<double>(width);
    display_height_ = static_cast<double>(height);
    FX_LOGS(INFO) << "Got display_width = " << display_width_
                  << " and display_height = " << display_height_;
  }

  void TearDown() override {
    bool complete = false;
    ui_test_manager_->TeardownRealm(
        [&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  }

  float ClientViewScaleFactor() { return ui_test_manager_->ClientViewScaleFactor(); }

  ui_testing::Screenshot TakeScreenshot() { return ui_test_manager_->TakeScreenshot(); }

  std::shared_ptr<ui_testing::TestViewAccess> test_view_access_;

  double display_width_ = 0;
  double display_height_ = 0;

 private:
  std::optional<ui_testing::UITestManager> ui_test_manager_;

  std::unique_ptr<sys::ServiceDirectory> realm_exposed_services_;
  std::optional<Realm> realm_;
};

namespace {

std::string GetDisplayPixelRatioTestName(
    const ::testing::TestParamInfo<DisplayPixelRatioTest::ParamType>& info) {
  const auto& [pixel_ratio, display_config] = info.param;

  std::string pixel_ratio_string = std::format("{:.2f}", pixel_ratio);
  size_t pos_point = pixel_ratio_string.find(".");
  if (pos_point != std::string::npos) {
    pixel_ratio_string.replace(pos_point, 1, "p");
  }

  return std::format("{}x{}x{}HzxPixelRatio{}", display_config.width_px, display_config.height_px,
                     display_config.refresh_rate_millihertz / 1'000, pixel_ratio_string);
}

}  // namespace

INSTANTIATE_TEST_SUITE_P(
    DisplayPixelRatioTestWithParams, DisplayPixelRatioTest,
    ::testing::Combine(::testing::Values(ui_testing::kDefaultDevicePixelRatio,
                                         ui_testing::kMediumResolutionDevicePixelRatio,
                                         ui_testing::kHighResolutionDevicePixelRatio),
                       ::testing::Values(kSherlockDisplayConfig)),
    GetDisplayPixelRatioTestName);

// This test leverage the coordinate test view to ensure that display pixel ratio is working
// properly.
// ___________________________________
// |                |                |
// |     BLACK      |        BLUE    |
// |           _____|_____           |
// |___________|  GREEN  |___________|
// |           |_________|           |
// |                |                |
// |      RED       |     MAGENTA    |
// |________________|________________|
TEST_P(DisplayPixelRatioTest, TestScale) {
  const auto expected_dpr = std::get<0>(GetParam());

  // TODO(https://fxbug.dev/42064286): Run this check when we update ClientScaleFactor()
  // to work with Flatland.
  // EXPECT_NEAR(ClientViewScaleFactor(), expected_dpr, kEpsilon);
  EXPECT_NEAR(display_width_ / test_view_access_->view()->width(), expected_dpr, kEpsilon);
  EXPECT_NEAR(display_height_ / test_view_access_->view()->height(), expected_dpr, kEpsilon);

  // The drawn content should cover the screen's display.
  auto data = TakeScreenshot();

  // Check pixel content at all four corners.
  EXPECT_EQ(data.GetPixelAt(0, 0), utils::kBlack);                 // Top left
  EXPECT_EQ(data.GetPixelAt(0, data.height() - 1), utils::kBlue);  // Bottom left
  EXPECT_EQ(data.GetPixelAt(data.width() - 1, 0), utils::kRed);    // Top right
  EXPECT_EQ(data.GetPixelAt(data.width() - 1, data.height() - 1),
            utils::kMagenta);  // Bottom right

  // Check pixel content at center of each rectangle.
  EXPECT_EQ(data.GetPixelAt(data.width() / 4, data.height() / 4),
            utils::kBlack);  // Top left
  EXPECT_EQ(data.GetPixelAt(data.width() / 4, (3 * data.height()) / 4),
            utils::kBlue);  // Bottom left
  EXPECT_EQ(data.GetPixelAt((3 * data.width()) / 4, data.height() / 4),
            utils::kRed);  // Top right
  EXPECT_EQ(data.GetPixelAt((3 * data.width()) / 4, (3 * data.height()) / 4),
            utils::kMagenta);  // Bottom right
  EXPECT_EQ(data.GetPixelAt(data.width() / 2, data.height() / 2),
            utils::kGreen);  // Center
}

TEST_P(DisplayPixelRatioTest, TestPixelColorDistribution) {
  auto data = TakeScreenshot();

  // Width and height of the rectangle in the center is |display_width_|/4 and |display_height_|/4.
  const auto expected_green_pixels = (display_height_ / 4) * (display_width_ / 4);

  // The number of pixels inside each quadrant would be |display_width_|/2 * |display_height_|/2.
  // Since the central rectangle would cover equal areas in each quadrant, we subtract
  // |expected_green_pixels|/4 from each quadrants to get the pixel for the quadrant's color.
  const auto expected_black_pixels =
      (display_height_ / 2) * (display_width_ / 2) - (expected_green_pixels / 4);

  const auto expected_blue_pixels = expected_black_pixels,
             expected_red_pixels = expected_black_pixels,
             expected_magenta_pixels = expected_black_pixels;

  auto histogram = data.Histogram();

  EXPECT_EQ(histogram[utils::kBlack], expected_black_pixels);
  EXPECT_EQ(histogram[utils::kBlue], expected_blue_pixels);
  EXPECT_EQ(histogram[utils::kRed], expected_red_pixels);
  EXPECT_EQ(histogram[utils::kMagenta], expected_magenta_pixels);
  EXPECT_EQ(histogram[utils::kGreen], expected_green_pixels);
}

}  // namespace integration_tests
