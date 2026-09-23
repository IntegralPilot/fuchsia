// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/logger/cpp/fidl.h>
#include <fuchsia/sysmem/cpp/fidl.h>
#include <fuchsia/sysmem2/cpp/fidl.h>
#include <fuchsia/tracing/provider/cpp/fidl.h>
#include <fuchsia/ui/app/cpp/fidl.h>
#include <fuchsia/ui/input3/cpp/fidl.h>
#include <fuchsia/vulkan/loader/cpp/fidl.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

#include "src/ui/testing/util/portable_ui_test.h"
#include "src/ui/testing/util/screenshot_helper.h"

namespace integration_tests {

using component_testing::ChildRef;
using component_testing::Dictionary;
using component_testing::ParentRef;
using component_testing::Protocol;

constexpr zx::duration kPredicateTimeout = zx::sec(10);

// Launches a carnelian app which draws a solid filled square on the display and takes a screenshot
// to validate content.
class CarnelianPixelTest : public ui_testing::PortableUITest {
 public:
  std::string GetTestUIStackUrl() override { return "#meta/test-ui-stack.cm"; }

 private:
  void ExtendRealm() override {
    // Add the carnelian component.
    realm_builder().AddChild(kCarnelianClient, kCarnelianClientUrl);

    realm_builder().AddRoute(
        {.capabilities = {Protocol{fuchsia::logger::LogSink::Name_},
                          Protocol{fuchsia::sysmem::Allocator::Name_},
                          Protocol{fuchsia::sysmem2::Allocator::Name_},
                          Protocol{fuchsia::tracing::provider::Registry::Name_},
                          Protocol{fuchsia::vulkan::loader::Loader::Name_}},
         .source = ParentRef(),
         .targets = {ChildRef{kCarnelianClient}}});

    realm_builder().AddRoute({.capabilities = {Dictionary{"diagnostics"}},
                              .source = ParentRef(),
                              .targets = {ChildRef{kCarnelianClient}, kTestUIStackRef}});

    realm_builder().AddRoute(
        {.capabilities = {Protocol{fuchsia::ui::composition::FlatlandFactory::Name_},
                          Protocol{fuchsia::ui::composition::Allocator::Name_},
                          Protocol{fuchsia::ui::input3::Keyboard::Name_}},
         .source = kTestUIStackRef,
         .targets = {ChildRef{kCarnelianClient}}});

    realm_builder().AddRoute({.capabilities = {Protocol{fuchsia::ui::app::ViewProvider::Name_}},
                              .source = ChildRef{kCarnelianClient},
                              .targets = {ParentRef()}});
  }

  static constexpr auto kCarnelianClient = "carnelian_client";
  static constexpr auto kCarnelianClientUrl = "#meta/static_square.cm";
};

// The carnelian app draws a scene as shown below:-
// The square is drawn in the center of the display with length |display_size().height/2|.
//   __________________________________
//  |                                 |
//  |              BLUE               |
//  |            ---------            |
//  |           |         |           |
//  |           | MAGENTA |           |
//  |            ---------            |
//  |                                 |
//  |_________________________________|
TEST_F(CarnelianPixelTest, ValidPixelTest) {
  const uint32_t total_pixels = display_size().width() * display_size().height();
  const uint32_t square_pixels = display_size().height() / 2 * display_size().height() / 2;
  const uint32_t background_pixels = total_pixels - square_pixels;

  LaunchClient();

  // Take screenshots till the background color shows up.
  ASSERT_TRUE(TakeScreenshotUntil(
      [&](const ui_testing::Screenshot& screenshot) {
        auto histogram = screenshot.Histogram();
        if (histogram[utils::kBlue] == 0)
          return false;
        // TODO(https://fxbug.dev/42067818): Switch to exact comparisons after Astro
        // precision issues are resolved.
        EXPECT_NEAR(histogram[utils::kMagenta], square_pixels, display_size().width());
        EXPECT_NEAR(histogram[utils::kBlue], background_pixels, display_size().width());
        return true;
      },
      kPredicateTimeout));
}

}  // namespace integration_tests
