// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_TESTS_UTILS_SCENIC_CTF_TEST_BASE_H_
#define SRC_UI_SCENIC_TESTS_UTILS_SCENIC_CTF_TEST_BASE_H_

#include <fidl/fuchsia.testing.harness/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.context/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/natural_types.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/zx/channel.h>

#include <cstdint>
#include <cstdlib>
#include <iostream>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/scenic_ctf_test_environment.h"
#include "src/ui/testing/util/logging_event_loop.h"

namespace integration_tests {

/// ScenicCtfTest use realm_proxy to connect scenic test realm.
/// The scenic test realm consists of three components:
///   * Scenic
///   * Fake Cobalt
///   * Fake Display Provider
///
/// topology as follows:
///       test_manager
///            |
///     <test component>
///            |                  <- Test realm
/// ----------------------------  <- realm_proxy
///     /      |     \            <- Scenic realm
///  Scenic  Cobalt  Hdcp
class ScenicCtfTest : public zxtest::Test, public ui_testing::LoggingEventLoop {
 public:
  ScenicCtfTest() = default;
  ~ScenicCtfTest() override = default;

  void SetUp() override { zxtest::Test::SetUp(); }

  // Delegates to the global test environment.
  void SetFlatlandDisplayContent(fuchsia_ui_views::ViewportCreationToken token);
  void SetFlatlandDisplayDevicePixelRatio(fuchsia_math::VecF dpr);

  const std::shared_ptr<sys::ServiceDirectory>& LocalServiceDirectory() const;

  /// Override GetDisplayRotation() to provide fuchsia.scenic.DisplayRotation to test realm. By
  /// default, it returns 0.
  virtual uint64_t GetDisplayRotation() const;

  /// Override GetDisplayDimensions() to provide `active_width_px` and `active_height_px` to
  /// fake-display-stack-host in the test realm. If {0, 0}, the default value will be used.
  /// By default, it returns {0, 0};
  ///
  /// `width` and `height` must be both non-zero or both zero.
  virtual fuchsia_math::SizeU GetDisplayDimensions() const;

  /// Override GetDisplayRefreshRateMillihertz() to provide `refresh_rate_millihertz` to
  /// fake-display-stack-host. If zero, the default value will be used. By default it returns zero.
  virtual uint32_t GetDisplayRefreshRateMillihertz() const;

  /// Override GetDisplayMaxLayerCount() to provide `max_layer_count` to
  /// fake-display-stack-host. If zero, the default value will be used. By default it returns zero.
  virtual uint32_t GetDisplayMaxLayerCount() const;

  /// Override UseDisplayComposition() to provide fuchsia.scenic.DisplayComposition to test realm.
  /// True by default.
  virtual bool UseDisplayComposition() const;

  /// Connect to the FIDL protocol which served from the realm proxy use default served path if no
  /// name passed in.
  template <typename Protocol>
  fidl::SyncClient<Protocol> ConnectSyncIntoRealm(
      const std::string& service_path = Protocol::kDiscoverableName) {
    return fidl::SyncClient<Protocol>(ConnectIntoRealm<Protocol>(service_path));
  }

  /// Connect to the FIDL protocol which served from the realm proxy use default served path if no
  /// name passed in.
  template <typename Protocol>
  fidl::Client<Protocol> ConnectAsyncIntoRealm(
      const std::string& service_path = Protocol::kDiscoverableName) {
    return fidl::Client<Protocol>(ConnectIntoRealm<Protocol>(service_path), dispatcher());
  }

  /// Connect to the FIDL protocol which served from the realm proxy use default served path if no
  /// name passed in.
  template <typename Protocol>
  fidl::ClientEnd<Protocol> ConnectIntoRealm(
      const std::string& service_path = Protocol::kDiscoverableName) {
    auto [client_end, server_end] = fidl::CreateEndpoints<Protocol>().value();

    auto& realm_proxy = ScenicCtfTestEnvironment::GetGlobalTestEnvironment()->realm_proxy();
    auto result = realm_proxy->ConnectToNamedProtocol(
        fuchsia_testing_harness::RealmProxyConnectToNamedProtocolRequest(service_path,
                                                                         server_end.TakeChannel()));
    if (result.is_error()) {
      std::cerr << "ConnectToNamedProtocol(" << service_path << ", " << Protocol::kDiscoverableName
                << ") failed." << std::endl;
      std::abort();
    }
    return std::move(client_end);
  }
};

}  // namespace integration_tests

#endif  // SRC_UI_SCENIC_TESTS_UTILS_SCENIC_CTF_TEST_BASE_H_
