// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/backlight-fidl-adapter.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fit/result.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <optional>
#include <utility>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-protocols/cpp/mock-backlight.h"
#include "src/graphics/display/lib/api-types/cpp/backlight-state.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

class BacklightFidlAdapterTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto [fidl_client, fidl_server] = fidl::Endpoints<fuchsia_hardware_backlight::Device>::Create();
    fidl_client_.Bind(std::move(fidl_client));

    ASSERT_OK(loop_.StartThread("mock-backlight-fidl-server"));
    ASSERT_OK(
        async::PostTask(loop_.dispatcher(), [this, fidl_server = std::move(fidl_server)]() mutable {
          fidl_adapter_.emplace(&mock_);
          fidl::ProtocolHandler<fuchsia_hardware_backlight::Device> fidl_handler =
              fidl_adapter_->CreateHandler(*loop_.dispatcher());
          fidl_handler(std::move(fidl_server));
        }));
  }

  void TearDown() override {
    ASSERT_OK(async::PostTask(loop_.dispatcher(), [this]() { fidl_adapter_.reset(); }));

    loop_.Shutdown();
    mock_.CheckAllCallsReplayed();
  }

 protected:
  display::testing::MockBacklight mock_;
  fidl::WireSyncClient<fuchsia_hardware_backlight::Device> fidl_client_;

  // Must be created and destroyed on the FIDL loop.
  std::optional<display::BacklightFidlAdapter> fidl_adapter_;

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

TEST_F(BacklightFidlAdapterTest, GetMaxBrightnessNitsSuccess) {
  mock_.ExpectGetMaxBrightnessNits([]() -> zx::result<float> { return zx::ok(500.0f); });

  fidl::WireResult<fuchsia_hardware_backlight::Device::GetMaxAbsoluteBrightness>
      fidl_transport_result = fidl_client_->GetMaxAbsoluteBrightness();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t,
              fuchsia_hardware_backlight::wire::DeviceGetMaxAbsoluteBrightnessResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_OK(fidl_domain_result);
  EXPECT_EQ(fidl_domain_result->max_brightness, 500.0);
}

TEST_F(BacklightFidlAdapterTest, GetMaxBrightnessNitsNotSupported) {
  mock_.ExpectGetMaxBrightnessNits(
      []() -> zx::result<float> { return zx::error(ZX_ERR_NOT_SUPPORTED); });

  fidl::WireResult<fuchsia_hardware_backlight::Device::GetMaxAbsoluteBrightness>
      fidl_transport_result = fidl_client_->GetMaxAbsoluteBrightness();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t,
              fuchsia_hardware_backlight::wire::DeviceGetMaxAbsoluteBrightnessResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(fidl_domain_result, zx::error(ZX_ERR_NOT_SUPPORTED));
}

TEST_F(BacklightFidlAdapterTest, GetStateNormalizedOnSuccess) {
  mock_.ExpectGetBacklightState([]() -> zx::result<BacklightState> {
    return zx::ok(BacklightState({
        .on = true,
        .brightness_fraction = 0.5,
        .brightness_nits = 400.0,
    }));
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::GetStateNormalized> fidl_transport_result =
      fidl_client_->GetStateNormalized();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_backlight::wire::DeviceGetStateNormalizedResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_OK(fidl_domain_result);
  EXPECT_EQ(fidl_domain_result->state.backlight_on, true);
  EXPECT_EQ(fidl_domain_result->state.brightness, 0.5);
}

TEST_F(BacklightFidlAdapterTest, GetStateNormalizedOffSuccess) {
  mock_.ExpectGetBacklightState([]() -> zx::result<BacklightState> {
    return zx::ok(BacklightState({
        .on = false,
        .brightness_fraction = 0.0,
        .brightness_nits = 0.0,
    }));
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::GetStateNormalized> fidl_transport_result =
      fidl_client_->GetStateNormalized();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_backlight::wire::DeviceGetStateNormalizedResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_OK(fidl_domain_result);
  EXPECT_EQ(fidl_domain_result->state.backlight_on, false);
  EXPECT_EQ(fidl_domain_result->state.brightness, 0.0);
}

TEST_F(BacklightFidlAdapterTest, GetStateNormalizedError) {
  mock_.ExpectGetBacklightState(
      []() -> zx::result<BacklightState> { return zx::error(ZX_ERR_IO); });
  fidl::WireResult<fuchsia_hardware_backlight::Device::GetStateNormalized> fidl_transport_result =
      fidl_client_->GetStateNormalized();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_backlight::wire::DeviceGetStateNormalizedResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(fidl_domain_result, zx::error(ZX_ERR_IO));
}

TEST_F(BacklightFidlAdapterTest, SetStateNormalizedOffSuccess) {
  mock_.ExpectSetBacklightState([](const BacklightState& state) -> zx::result<> {
    EXPECT_EQ(state.on(), false);
    EXPECT_EQ(state.brightness_fraction(), 0.0);
    EXPECT_EQ(state.brightness_nits(), std::nullopt);
    return zx::ok();
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::SetStateNormalized> fidl_transport_result =
      fidl_client_->SetStateNormalized(fuchsia_hardware_backlight::wire::State{
          .backlight_on = false,
          .brightness = 0.0,
      });
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(BacklightFidlAdapterTest, SetStateNormalizedOnSuccess) {
  mock_.ExpectSetBacklightState([](const BacklightState& state) -> zx::result<> {
    EXPECT_EQ(state.on(), true);
    EXPECT_EQ(state.brightness_fraction(), 0.5);
    EXPECT_EQ(state.brightness_nits(), std::nullopt);
    return zx::ok();
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::SetStateNormalized> fidl_transport_result =
      fidl_client_->SetStateNormalized(fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = 0.5,
      });
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(BacklightFidlAdapterTest, SetStateNormalizedHardwareError) {
  mock_.ExpectSetBacklightState([](const BacklightState& state) -> zx::result<> {
    EXPECT_EQ(state.brightness_nits(), std::nullopt);
    return zx::error(ZX_ERR_IO);
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::SetStateNormalized> fidl_transport_result =
      fidl_client_->SetStateNormalized(fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = 0.5,
      });
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(fidl_domain_result, zx::error(ZX_ERR_IO));
}

TEST_F(BacklightFidlAdapterTest, SetStateNormalizedOffParameterError) {
  fidl::WireResult<fuchsia_hardware_backlight::Device::SetStateNormalized> fidl_transport_result =
      fidl_client_->SetStateNormalized(fuchsia_hardware_backlight::wire::State{
          .backlight_on = false,
          .brightness = -1.0,
      });
  ASSERT_FALSE(fidl_transport_result.ok());
  EXPECT_EQ(fidl_transport_result.error().reason(), fidl::Reason::kPeerClosedWhileReading)
      << fidl_transport_result.FormatDescription();
}

TEST_F(BacklightFidlAdapterTest, GetStateAbsoluteOnSuccess) {
  mock_.ExpectGetBacklightState([]() -> zx::result<BacklightState> {
    return zx::ok(BacklightState({
        .on = true,
        .brightness_fraction = 0.5,
        .brightness_nits = 400.0,
    }));
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::GetStateAbsolute> fidl_transport_result =
      fidl_client_->GetStateAbsolute();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_backlight::wire::DeviceGetStateAbsoluteResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_OK(fidl_domain_result);
  EXPECT_EQ(fidl_domain_result->state.backlight_on, true);
  EXPECT_EQ(fidl_domain_result->state.brightness, 400.0);
}

TEST_F(BacklightFidlAdapterTest, GetStateAbsoluteOffSuccess) {
  mock_.ExpectGetBacklightState([]() -> zx::result<BacklightState> {
    return zx::ok(
        BacklightState({.on = false, .brightness_fraction = 0.0, .brightness_nits = 0.0}));
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::GetStateAbsolute> fidl_transport_result =
      fidl_client_->GetStateAbsolute();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_backlight::wire::DeviceGetStateAbsoluteResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_OK(fidl_domain_result);
  EXPECT_EQ(fidl_domain_result->state.backlight_on, false);
  EXPECT_EQ(fidl_domain_result->state.brightness, 0.0);
}

TEST_F(BacklightFidlAdapterTest, GetStateAbsoluteOnNotSupported) {
  mock_.ExpectGetBacklightState([]() -> zx::result<BacklightState> {
    return zx::ok(BacklightState({
        .on = true,
        .brightness_fraction = 0.5,
        .brightness_nits = std::nullopt,
    }));
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::GetStateAbsolute> fidl_transport_result =
      fidl_client_->GetStateAbsolute();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_backlight::wire::DeviceGetStateAbsoluteResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(fidl_domain_result, zx::error(ZX_ERR_NOT_SUPPORTED));
}

TEST_F(BacklightFidlAdapterTest, GetStateAbsoluteOffNotSupported) {
  mock_.ExpectGetBacklightState([]() -> zx::result<BacklightState> {
    return zx::ok(BacklightState({
        .on = false,
        .brightness_fraction = 0.0,
        .brightness_nits = std::nullopt,
    }));
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::GetStateAbsolute> fidl_transport_result =
      fidl_client_->GetStateAbsolute();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_backlight::wire::DeviceGetStateAbsoluteResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(fidl_domain_result, zx::error(ZX_ERR_NOT_SUPPORTED));
}

TEST_F(BacklightFidlAdapterTest, GetStateAbsoluteError) {
  mock_.ExpectGetBacklightState(
      []() -> zx::result<BacklightState> { return zx::error(ZX_ERR_IO); });
  fidl::WireResult<fuchsia_hardware_backlight::Device::GetStateAbsolute> fidl_transport_result =
      fidl_client_->GetStateAbsolute();
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_backlight::wire::DeviceGetStateAbsoluteResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(fidl_domain_result, zx::error(ZX_ERR_IO));
}

TEST_F(BacklightFidlAdapterTest, SetStateAbsoluteOffSuccess) {
  mock_.ExpectSetBacklightState([](const BacklightState& state) -> zx::result<> {
    EXPECT_EQ(state.on(), false);
    EXPECT_EQ(state.brightness_fraction(), 0.0);
    EXPECT_EQ(state.brightness_nits(), 0.0);
    return zx::ok();
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::SetStateAbsolute> fidl_transport_result =
      fidl_client_->SetStateAbsolute(fuchsia_hardware_backlight::wire::State{
          .backlight_on = false,
          .brightness = 0.0,
      });
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(BacklightFidlAdapterTest, SetStateAbsoluteOnSuccess) {
  mock_.ExpectSetBacklightState([](const BacklightState& state) -> zx::result<> {
    EXPECT_EQ(state.on(), true);
    EXPECT_EQ(state.brightness_fraction(), 0.0);
    EXPECT_EQ(state.brightness_nits(), 400.0);
    return zx::ok();
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::SetStateAbsolute> fidl_transport_result =
      fidl_client_->SetStateAbsolute(fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = 400.0,
      });
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(BacklightFidlAdapterTest, SetStateAbsoluteHardwareError) {
  mock_.ExpectSetBacklightState([](const BacklightState& state) -> zx::result<> {
    EXPECT_NE(state.brightness_nits(), std::nullopt);
    return zx::error(ZX_ERR_IO);
  });

  fidl::WireResult<fuchsia_hardware_backlight::Device::SetStateAbsolute> fidl_transport_result =
      fidl_client_->SetStateAbsolute(fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = 400.0,
      });
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(fidl_domain_result, zx::error(ZX_ERR_IO));
}

TEST_F(BacklightFidlAdapterTest, SetStateAbsoluteOffParameterError) {
  fidl::WireResult<fuchsia_hardware_backlight::Device::SetStateAbsolute> fidl_transport_result =
      fidl_client_->SetStateAbsolute(fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = -1.0,
      });
  ASSERT_FALSE(fidl_transport_result.ok());
  EXPECT_EQ(fidl_transport_result.error().reason(), fidl::Reason::kPeerClosedWhileReading)
      << fidl_transport_result.FormatDescription();
}

}  // namespace

}  // namespace display
