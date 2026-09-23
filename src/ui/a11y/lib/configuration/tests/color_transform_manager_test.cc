// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/configuration/color_transform_manager.h"

#include <fidl/fuchsia.accessibility/cpp/fidl.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>

#include <cmath>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/test_loop_fixture.h>

namespace accessibility_test {
namespace {

// clang-format off
const std::array<float, 9> kIdentityMatrix = {
    1, 0, 0,
    0, 1, 0,
    0, 0, 1};
const std::array<float, 3> kZero3x1Vector = {0, 0, 0};

const std::array<float, 9> kColorInversionMatrix = {
    0.402f,  -1.174f,  -0.228f,
   -0.598f,  -0.174f,  -0.228f,
   -0.599f,  -1.177f,   0.771f};
const std::array<float, 3> kColorInversionPostOffset = {.999f, .999f, .999f};

const std::array<float, 9> kCorrectProtanomaly = {
    0.622774, 0.377226,  0.000000,
    0.264275, 0.735725,  -0.000000,
    0.216821, -0.216821, 1.000000};

const std::array<float, 9> kProtanomalyAndInversionMatrix = {
    -0.10933889, -0.66266111, -0.228,
    -0.46783789, -0.30416211, -0.228,
    -0.51692431, -1.25907569,  0.771};
const std::array<float, 3> kProtanomalyAndInversionPostOffset = {.999f, .999f, .999f};

// clang-format on

class FakeColorTransformHandler
    : public fidl::Server<fuchsia_accessibility::ColorTransformHandler> {
 public:
  FakeColorTransformHandler() = default;
  ~FakeColorTransformHandler() = default;

  void Serve(async_dispatcher_t* dispatcher) {
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_accessibility::ColorTransformHandler>::Create();
    bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
    client_end_ = std::move(client_end);
  }

  // |fuchsia.accessibility.ColorTransformHandler|
  void SetColorTransformConfiguration(
      SetColorTransformConfigurationRequest& req,
      SetColorTransformConfigurationCompleter::Sync& completer) override {
    transform_ = req.configuration().color_adjustment_matrix().has_value()
                     ? *req.configuration().color_adjustment_matrix()
                     : kIdentityMatrix;
    pre_offset_ = req.configuration().color_adjustment_pre_offset().has_value()
                      ? *req.configuration().color_adjustment_pre_offset()
                      : kZero3x1Vector;
    post_offset_ = req.configuration().color_adjustment_post_offset().has_value()
                       ? *req.configuration().color_adjustment_post_offset()
                       : kZero3x1Vector;

    color_inversion_enabled_ = req.configuration().color_inversion_enabled().has_value()
                                   ? req.configuration().color_inversion_enabled()
                                   : false;

    color_correction_mode_ = req.configuration().color_correction().has_value()
                                 ? *req.configuration().color_correction()
                                 : fuchsia_accessibility::ColorCorrectionMode::kDisabled;
    completer.Reply();
  }

  bool hasTransform(std::array<float, 9> transform_to_compare) const {
    return FloatArraysAreEqual(transform_, transform_to_compare);
  }

  bool hasPostOffset(std::array<float, 3> offset_to_compare) const {
    return FloatArraysAreEqual(post_offset_, offset_to_compare);
  }

  std::array<float, 9> transform_;
  std::array<float, 3> pre_offset_;
  std::array<float, 3> post_offset_;

  // These fields `has_value()` iff SetColorTransformConfiguration() has been called.
  std::optional<bool> color_inversion_enabled_;
  std::optional<fuchsia_accessibility::ColorCorrectionMode> color_correction_mode_;
  fidl::ClientEnd<fuchsia_accessibility::ColorTransformHandler> client_end_;

  fidl::ServerBindingGroup<fuchsia_accessibility::ColorTransformHandler> bindings_;

 private:
  template <size_t N>
  static bool FloatArraysAreEqual(const std::array<float, N>& a, const std::array<float, N>& b) {
    const float float_comparison_epsilon = 0.00001;
    for (size_t i = 0; i < N; i++) {
      if ((std::fabs(a[i] - b[i]) > float_comparison_epsilon)) {
        return false;
      }
    }
    return true;
  }
};

class ColorTransformManagerTest : public gtest::TestLoopFixture {
 public:
  ColorTransformManagerTest() = default;
  ~ColorTransformManagerTest() override = default;

  void SetUp() override {
    TestLoopFixture::SetUp();

    color_transform_handler_.Serve(dispatcher());

    startup_context_ = sys::ComponentContext::CreateAndServeOutgoingDirectory();

    color_transform_manager_ =
        std::make_unique<a11y::ColorTransformManager>(dispatcher(), startup_context_.get());

    auto [color_transform_client_end, color_transform_server_end] =
        fidl::Endpoints<fuchsia_accessibility::ColorTransform>::Create();
    bindings_.AddBinding(dispatcher(), std::move(color_transform_server_end),
                         color_transform_manager_.get(), fidl::kIgnoreBindingClosure);

    color_transform_ = fidl::SyncClient(std::move(color_transform_client_end));

    RunLoopUntilIdle();
  }

  std::unique_ptr<sys::ComponentContext> startup_context_;
  std::unique_ptr<a11y::ColorTransformManager> color_transform_manager_;
  FakeColorTransformHandler color_transform_handler_;
  fidl::SyncClient<fuchsia_accessibility::ColorTransform> color_transform_;

  fidl::ServerBindingGroup<fuchsia_accessibility::ColorTransform> bindings_;
};

TEST_F(ColorTransformManagerTest, NoHandler) {
  // change a setting
  color_transform_manager_->ChangeColorTransform(
      false, fuchsia_accessibility::ColorCorrectionMode::kDisabled);
  RunLoopUntilIdle();

  // This test is verifying that nothing crashes.
}

TEST_F(ColorTransformManagerTest, SetColorTransformDefault) {
  // register a (fake) handler
  auto res = color_transform_->RegisterColorTransformHandler(
      {std::move(color_transform_handler_.client_end_)});
  ASSERT_TRUE(res.is_ok());

  // change a setting
  color_transform_manager_->ChangeColorTransform(
      false, fuchsia_accessibility::ColorCorrectionMode::kDisabled);
  RunLoopUntilIdle();

  // Verify handler gets sent the correct settings.
  ASSERT_TRUE(color_transform_handler_.color_inversion_enabled_.has_value());
  EXPECT_FALSE(color_transform_handler_.color_inversion_enabled_.value());
  ASSERT_TRUE(color_transform_handler_.color_correction_mode_.has_value());
  EXPECT_EQ(color_transform_handler_.color_correction_mode_.value(),
            fuchsia_accessibility::ColorCorrectionMode::kDisabled);
  EXPECT_TRUE(color_transform_handler_.hasTransform(kIdentityMatrix));
}

TEST_F(ColorTransformManagerTest, SetColorInversionEnabled) {
  // register a (fake) handler
  auto res = color_transform_->RegisterColorTransformHandler(
      {std::move(color_transform_handler_.client_end_)});
  ASSERT_TRUE(res.is_ok());

  // change a setting
  color_transform_manager_->ChangeColorTransform(
      true, fuchsia_accessibility::ColorCorrectionMode::kDisabled);
  RunLoopUntilIdle();

  // Verify handler gets sent the correct settings.
  ASSERT_TRUE(color_transform_handler_.color_inversion_enabled_.has_value());
  EXPECT_TRUE(color_transform_handler_.color_inversion_enabled_.value());
  ASSERT_TRUE(color_transform_handler_.color_correction_mode_.has_value());
  EXPECT_EQ(color_transform_handler_.color_correction_mode_.value(),
            fuchsia_accessibility::ColorCorrectionMode::kDisabled);
  EXPECT_TRUE(color_transform_handler_.hasTransform(kColorInversionMatrix));
  EXPECT_TRUE(color_transform_handler_.hasPostOffset(kColorInversionPostOffset));
}

TEST_F(ColorTransformManagerTest, SetColorCorrection) {
  // register a (fake) handler
  auto res = color_transform_->RegisterColorTransformHandler(
      {std::move(color_transform_handler_.client_end_)});
  ASSERT_TRUE(res.is_ok());

  // change a setting
  color_transform_manager_->ChangeColorTransform(
      false, fuchsia_accessibility::ColorCorrectionMode::kCorrectProtanomaly);
  RunLoopUntilIdle();

  // Verify handler gets sent the correct settings.
  ASSERT_TRUE(color_transform_handler_.color_inversion_enabled_.has_value());
  EXPECT_FALSE(color_transform_handler_.color_inversion_enabled_.value());
  ASSERT_TRUE(color_transform_handler_.color_correction_mode_.has_value());
  EXPECT_EQ(color_transform_handler_.color_correction_mode_.value(),
            fuchsia_accessibility::ColorCorrectionMode::kCorrectProtanomaly);
  EXPECT_TRUE(color_transform_handler_.hasTransform(kCorrectProtanomaly));
}

TEST_F(ColorTransformManagerTest, SetColorCorrectionAndInversion) {
  // register a (fake) handler
  auto res = color_transform_->RegisterColorTransformHandler(
      {std::move(color_transform_handler_.client_end_)});
  ASSERT_TRUE(res.is_ok());

  // change a setting
  color_transform_manager_->ChangeColorTransform(
      true, fuchsia_accessibility::ColorCorrectionMode::kCorrectProtanomaly);
  RunLoopUntilIdle();

  // Verify handler gets sent the correct settings.
  ASSERT_TRUE(color_transform_handler_.color_inversion_enabled_.has_value());
  EXPECT_TRUE(color_transform_handler_.color_inversion_enabled_.value());
  ASSERT_TRUE(color_transform_handler_.color_correction_mode_.has_value());
  EXPECT_EQ(color_transform_handler_.color_correction_mode_.value(),
            fuchsia_accessibility::ColorCorrectionMode::kCorrectProtanomaly);
  EXPECT_TRUE(color_transform_handler_.hasTransform(kProtanomalyAndInversionMatrix));
  EXPECT_TRUE(color_transform_handler_.hasPostOffset(kProtanomalyAndInversionPostOffset));
}

TEST_F(ColorTransformManagerTest, BuffersChangeBeforeHandlerRegistered) {
  // Enable color inversion and color correction.
  color_transform_manager_->ChangeColorTransform(
      true, fuchsia_accessibility::ColorCorrectionMode::kCorrectDeuteranomaly);
  RunLoopUntilIdle();

  // Register the (fake) handler.
  auto res = color_transform_->RegisterColorTransformHandler(
      {std::move(color_transform_handler_.client_end_)});
  ASSERT_TRUE(res.is_ok());
  RunLoopUntilIdle();

  // Verify handler gets sent the correct settings.
  ASSERT_TRUE(color_transform_handler_.color_inversion_enabled_.has_value());
  EXPECT_TRUE(color_transform_handler_.color_inversion_enabled_.value());
  ASSERT_TRUE(color_transform_handler_.color_correction_mode_.has_value());
  EXPECT_EQ(color_transform_handler_.color_correction_mode_.value(),
            fuchsia_accessibility::ColorCorrectionMode::kCorrectDeuteranomaly);
}

TEST_F(ColorTransformManagerTest, ReappliesSettingOnHandlerChange) {
  // Enable color inversion and color correction.
  color_transform_manager_->ChangeColorTransform(
      true, fuchsia_accessibility::ColorCorrectionMode::kCorrectDeuteranomaly);
  RunLoopUntilIdle();

  // Register the (fake) handler.
  auto res = color_transform_->RegisterColorTransformHandler(
      {std::move(color_transform_handler_.client_end_)});
  ASSERT_TRUE(res.is_ok());
  RunLoopUntilIdle();

  // Create and register a new (fake) handler.
  FakeColorTransformHandler new_handler;
  new_handler.Serve(dispatcher());
  res = color_transform_->RegisterColorTransformHandler({std::move(new_handler.client_end_)});
  ASSERT_TRUE(res.is_ok());
  RunLoopUntilIdle();

  // Verify the new handler gets sent the correct settings.
  ASSERT_TRUE(new_handler.color_inversion_enabled_.has_value());
  EXPECT_TRUE(new_handler.color_inversion_enabled_.value());
  ASSERT_TRUE(new_handler.color_correction_mode_.has_value());
  EXPECT_EQ(new_handler.color_correction_mode_.value(),
            fuchsia_accessibility::ColorCorrectionMode::kCorrectDeuteranomaly);
}

}  // namespace
}  // namespace accessibility_test
