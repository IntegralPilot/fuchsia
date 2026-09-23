// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTS_CONFORMANCE_INPUT_TESTS_CONFORMANCE_TEST_BASE_H_
#define SRC_UI_TESTS_CONFORMANCE_INPUT_TESTS_CONFORMANCE_TEST_BASE_H_

#include <fidl/fuchsia.testing.harness/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.context/cpp/fidl.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>

#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/testing/util/logging_event_loop.h>
#include <zxtest/zxtest.h>

#include "lib/fidl/cpp/wire/internal/transport_channel.h"

namespace ui_conformance_test_base {

/// ConformanceTest use realm_proxy to connect test realm.
class ConformanceTest : public zxtest::Test, public ui_testing::LoggingEventLoop {
 public:
  ConformanceTest() = default;
  ~ConformanceTest() override = default;

  /// SetUp connect test realm so test can use realm_proxy_ to access.
  void SetUp() override;

  /// Override DisplayRotation() to provide display_rotation to test realm.
  /// By default, it returns 0.
  virtual uint32_t DisplayRotation() const;

  /// Override DevicePixelRatio() to provide device_pixel_ratio to test realm.
  /// By default, it returns 1.0.
  virtual float DevicePixelRatio() const;

  /// ClientEnd connect to the FIDL protocol which served from the realm proxy
  /// use default served path if no name passed in.
  template <typename Protocol>
  fidl::ClientEnd<Protocol> ConnectIntoRealm(
      const std::string& service_path = fidl::DiscoverableProtocolName<Protocol>) {
    auto [client_end, server_end] = fidl::Endpoints<Protocol>::Create();

    ZX_ASSERT_OK(realm_proxy_->ConnectToNamedProtocol(
        fuchsia_testing_harness::RealmProxyConnectToNamedProtocolRequest(
            service_path, server_end.TakeChannel())));

    return std::move(client_end);
  }

  /// SyncClient connect to the FIDL protocol which served from the realm proxy
  /// use default served path if no name passed in.
  template <typename Protocol>
  fidl::SyncClient<Protocol> ConnectSyncIntoRealm(
      const std::string& service_path = fidl::DiscoverableProtocolName<Protocol>) {
    return fidl::SyncClient(ConnectIntoRealm<Protocol>(service_path));
  }

 private:
  fidl::SyncClient<fuchsia_ui_test_context::RealmFactory> realm_factory_;
  fidl::SyncClient<fuchsia_testing_harness::RealmProxy> realm_proxy_;
};

}  // namespace ui_conformance_test_base

#endif  // SRC_UI_TESTS_CONFORMANCE_INPUT_TESTS_CONFORMANCE_TEST_BASE_H_
