// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.testing.harness/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.context/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"

namespace integration_tests {

namespace {

using fuc_FlatlandDisplay = fuchsia_ui_composition::FlatlandDisplay;
using fuds_Metrics = fuchsia_ui_display_singleton::Metrics;
using fuds_Info = fuchsia_ui_display_singleton::Info;

// Max timeout in failure cases.
// Set this as low as you can that still works across all test platforms.
constexpr zx::duration kTimeout = zx::min(5);

struct DisplayConfig {
  uint32_t width;
  uint32_t height;
  uint32_t refresh_rate_millihertz;
};

class SingletonDisplayIntegrationTest : public zxtest::Test,
                                        public ui_testing::LoggingEventLoop,
                                        public zxtest::WithParamInterface<DisplayConfig> {
 public:
  SingletonDisplayIntegrationTest() = default;

  void SetUp() override {
    zxtest::Test::SetUp();
    {
      auto client_end = component::Connect<fuchsia_ui_test_context::ScenicRealmFactory>();
      ASSERT_TRUE(client_end.is_ok());
      realm_factory_ = fidl::SyncClient(std::move(client_end.value()));

      auto [realm_proxy_client_end, realm_proxy_server_end] =
          fidl::CreateEndpoints<fuchsia_testing_harness::RealmProxy>().value();

      fuchsia_ui_test_context::ScenicRealmFactoryCreateRealmRequest req;
      req.realm_server(std::move(realm_proxy_server_end));
      req.display_rotation(0);
      req.renderer(fuchsia_ui_test_context::RendererType::kNull);
      req.display_composition(true);
      if (GetDisplayDimensions().height() != 0 && GetDisplayDimensions().width() != 0) {
        req.display_dimensions(GetDisplayDimensions());
      }
      if (GetDisplayRefreshRateMillihertz() != 0) {
        req.display_refresh_rate_millihertz(GetDisplayRefreshRateMillihertz());
      }

      auto res = realm_factory_->CreateRealm(std::move(req));
      ASSERT_TRUE(res.is_ok());
      realm_proxy_ = fidl::SyncClient(std::move(realm_proxy_client_end));
    }

    // Post a "just in case" quit task, if the test hangs.
    async::PostDelayedTask(
        dispatcher(),
        [] { FX_LOGS(FATAL) << "\n\n>> Test did not complete in time, terminating.  <<\n\n"; },
        kTimeout);

    singleton_display_ = ConnectSyncIntoRealm<fuds_Info>();
  }

  fuchsia_math::SizeU GetDisplayDimensions() const {
    return {{.width = GetParam().width, .height = GetParam().height}};
  }
  uint32_t GetDisplayRefreshRateMillihertz() const { return GetParam().refresh_rate_millihertz; }

  /// Connect to the FIDL protocol which served from the realm proxy use default served path if no
  /// name passed in.
  template <typename Protocol>
  fidl::SyncClient<Protocol> ConnectSyncIntoRealm(
      const std::string& service_path = Protocol::kDiscoverableName) {
    auto [client_end, server_end] = fidl::CreateEndpoints<Protocol>().value();
    auto result = realm_proxy_->ConnectToNamedProtocol(
        fuchsia_testing_harness::RealmProxyConnectToNamedProtocolRequest(service_path,
                                                                         server_end.TakeChannel()));
    if (result.is_error()) {
      std::cerr << "ConnectToNamedProtocol(" << service_path << ", " << Protocol::kDiscoverableName
                << ") failed." << std::endl;
      std::abort();
    }
    return fidl::SyncClient<Protocol>(std::move(client_end));
  }

 protected:
  fidl::SyncClient<fuds_Info> singleton_display_;
  fidl::SyncClient<fuchsia_ui_test_context::ScenicRealmFactory> realm_factory_;
  fidl::SyncClient<fuchsia_testing_harness::RealmProxy> realm_proxy_;
};

TEST_P(SingletonDisplayIntegrationTest, GetMetrics) {
  auto result = singleton_display_->GetMetrics();
  ASSERT_TRUE(result.is_ok());
  const auto& metrics = result->info();

  ASSERT_TRUE(metrics.extent_in_px().has_value());
  ASSERT_TRUE(metrics.extent_in_mm().has_value());
  ASSERT_TRUE(metrics.recommended_device_pixel_ratio().has_value());

  EXPECT_EQ(GetDisplayDimensions().width(), metrics.extent_in_px()->width());
  EXPECT_EQ(GetDisplayDimensions().height(), metrics.extent_in_px()->height());
  EXPECT_EQ(160, metrics.extent_in_mm()->width());
  EXPECT_EQ(90, metrics.extent_in_mm()->height());
  EXPECT_EQ(1.f, metrics.recommended_device_pixel_ratio()->x());
  EXPECT_EQ(1.f, metrics.recommended_device_pixel_ratio()->y());
  EXPECT_EQ(GetDisplayRefreshRateMillihertz(), metrics.maximum_refresh_rate_in_millihertz());
}

TEST_P(SingletonDisplayIntegrationTest, DevicePixelRatioChange) {
  fidl::SyncClient flatland_display = ConnectSyncIntoRealm<fuc_FlatlandDisplay>();
  const float kDPRx = 1.25f;
  const float kDPRy = 1.25f;
  auto result =
      flatland_display->SetDevicePixelRatio({{.device_pixel_ratio = {{.x = kDPRx, .y = kDPRy}}}});
  ASSERT_TRUE(result.is_ok());

  // FlatlandDisplay lives on a Flatland thread and SingletonDisplay lives on the main thread, so
  // the update may not be sequential.
  RunLoopUntil([this, kDPRx, kDPRy] {
    auto res = singleton_display_->GetMetrics();
    EXPECT_TRUE(res.is_ok());
    if (!res.is_ok())
      return false;
    const auto& metrics = res->info();
    return metrics.recommended_device_pixel_ratio().has_value() &&
           kDPRx == metrics.recommended_device_pixel_ratio()->x() &&
           kDPRy == metrics.recommended_device_pixel_ratio()->y();
  });
}

constexpr DisplayConfig kSherlockDisplayConfig = {
    .width = 1280,
    .height = 800,
    .refresh_rate_millihertz = 60000,
};
constexpr DisplayConfig kAstroDisplayConfig = {
    .width = 1024,
    .height = 600,
    .refresh_rate_millihertz = 60000,
};
constexpr DisplayConfig kAstroLowRefreshRateDisplayConfig = {
    .width = 1024,
    .height = 600,
    .refresh_rate_millihertz = 30000,
};

INSTANTIATE_TEST_SUITE_P(Panel, SingletonDisplayIntegrationTest,
                         zxtest::Values(kAstroDisplayConfig, kSherlockDisplayConfig,
                                        kAstroLowRefreshRateDisplayConfig));

}  // namespace

}  // namespace integration_tests
