// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/color_converter.h"

#include <fidl/fuchsia.ui.display.color/cpp/fidl.h>
#include <lib/sys/cpp/testing/component_context_provider.h>

#include <cmath>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace display::test {

class ColorConverterTest : public gtest::TestLoopFixture {
 public:
  ColorConverterTest()
      : color_converter_(
            context_provider_.context(),
            [this](const fidl::Array<float, 9>& coefficients,
                   const fidl::Array<float, 3>& preoffsets,
                   const fidl::Array<float, 3>& postoffsets) {
              set_color_conversion_called_ = true;
              coefficients_ = coefficients;
              preoffsets_ = preoffsets;
              postoffsets_ = postoffsets;
            },
            [this](uint8_t minimum_rgb) {
              set_minimum_rgb_called_ = true;
              minimum_rgb_ = minimum_rgb;
              return minimum_rgb_return_value_;
            }) {}

 protected:
  fidl::Client<fuchsia_ui_display_color::Converter> Connect() {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_display_color::Converter>::Create();
    context_provider_.public_service_directory()->Connect(
        fidl::DiscoverableProtocolName<fuchsia_ui_display_color::Converter>,
        server_end.TakeChannel());
    return fidl::Client<fuchsia_ui_display_color::Converter>(std::move(client_end), dispatcher());
  }

  sys::testing::ComponentContextProvider context_provider_;
  ColorConverter color_converter_;

  bool set_color_conversion_called_ = false;
  fidl::Array<float, 9> coefficients_{};
  fidl::Array<float, 3> preoffsets_{};
  fidl::Array<float, 3> postoffsets_{};

  bool set_minimum_rgb_called_ = false;
  uint8_t minimum_rgb_ = 0;
  bool minimum_rgb_return_value_ = true;
};

TEST_F(ColorConverterTest, SetValues_Valid) {
  fidl::Client client = Connect();
  bool callback_called = false;

  fuchsia_ui_display_color::ConversionProperties props;
  props.coefficients(std::array<float, 9>{1.f, 2.f, 3.f, 4.f, 5.f, 6.f, 7.f, 8.f, 9.f});
  props.preoffsets(std::array<float, 3>{0.1f, 0.2f, 0.3f});
  props.postoffsets(std::array<float, 3>{0.4f, 0.5f, 0.6f});

  client->SetValues({std::move(props)})
      .Then([&](fidl::Result<fuchsia_ui_display_color::Converter::SetValues>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_EQ(result->res(), ZX_OK);
        callback_called = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_TRUE(set_color_conversion_called_);
  EXPECT_EQ(coefficients_, (fidl::Array<float, 9>{1.f, 2.f, 3.f, 4.f, 5.f, 6.f, 7.f, 8.f, 9.f}));
  EXPECT_EQ(preoffsets_, (fidl::Array<float, 3>{0.1f, 0.2f, 0.3f}));
  EXPECT_EQ(postoffsets_, (fidl::Array<float, 3>{0.4f, 0.5f, 0.6f}));
}

TEST_F(ColorConverterTest, SetValues_Defaults) {
  fidl::Client client = Connect();
  bool callback_called = false;

  fuchsia_ui_display_color::ConversionProperties empty_props;
  client->SetValues({std::move(empty_props)})
      .Then([&](fidl::Result<fuchsia_ui_display_color::Converter::SetValues>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_EQ(result->res(), ZX_OK);
        callback_called = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_TRUE(set_color_conversion_called_);
  EXPECT_EQ(coefficients_, (fidl::Array<float, 9>{1.f, 0.f, 0.f, 0.f, 1.f, 0.f, 0.f, 0.f, 1.f}));
  EXPECT_EQ(preoffsets_, (fidl::Array<float, 3>{0.f, 0.f, 0.f}));
  EXPECT_EQ(postoffsets_, (fidl::Array<float, 3>{0.f, 0.f, 0.f}));
}

TEST_F(ColorConverterTest, SetValues_InvalidValues) {
  fidl::Client client = Connect();
  bool callback_called = false;

  fuchsia_ui_display_color::ConversionProperties props;
  props.coefficients(std::array<float, 9>{NAN, 0.f, 0.f, 0.f, 1.f, 0.f, 0.f, 0.f, 1.f});

  client->SetValues({std::move(props)})
      .Then([&](fidl::Result<fuchsia_ui_display_color::Converter::SetValues>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_EQ(result->res(), ZX_ERR_INVALID_ARGS);
        callback_called = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_FALSE(set_color_conversion_called_);
}

TEST_F(ColorConverterTest, SetMinimumRgb) {
  fidl::Client client = Connect();
  bool callback_called = false;

  minimum_rgb_return_value_ = true;
  client->SetMinimumRgb({50}).Then(
      [&](fidl::Result<fuchsia_ui_display_color::Converter::SetMinimumRgb>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_TRUE(result->supported());
        callback_called = true;
      });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_TRUE(set_minimum_rgb_called_);
  EXPECT_EQ(minimum_rgb_, 50);
}

}  // namespace display::test
