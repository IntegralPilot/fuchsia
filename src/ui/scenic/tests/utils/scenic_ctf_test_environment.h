// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_TESTS_UTILS_SCENIC_CTF_TEST_ENVIRONMENT_H_
#define SRC_UI_SCENIC_TESTS_UTILS_SCENIC_CTF_TEST_ENVIRONMENT_H_

#include <fidl/fuchsia.testing.harness/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.context/cpp/fidl.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <zxtest/base/environment.h>
#include <zxtest/zxtest.h>

namespace integration_tests {

class ScenicCtfTestEnvironment : public zxtest::Environment {
 public:
  // Register ScenicCtfTestEnvironment as a zxtest global test environment. The
  // environment will be managed by zxtest after being registered.
  static void RegisterGlobalTestEnvironment(fuchsia_ui_test_context::RendererType renderer_type);
  static ScenicCtfTestEnvironment* GetGlobalTestEnvironment();

  /// SetUp connects the test realm so tests can use realm_proxy_ to access.
  void SetUp() override;
  void TearDown() override;

  void SetFlatlandDisplayContent(fuchsia_ui_views::ViewportCreationToken token);
  void SetFlatlandDisplayDevicePixelRatio(fuchsia_math::VecF dpr);

  const std::shared_ptr<sys::ServiceDirectory>& LocalServiceDirectory() const;

  uint64_t GetDisplayRotation() const;

  fuchsia_math::SizeU GetDisplayDimensions() const;

  uint32_t GetDisplayRefreshRateMillihertz() const;

  uint32_t GetDisplayMaxLayerCount() const;

  bool UseDisplayComposition() const;

  fidl::SyncClient<fuchsia_testing_harness::RealmProxy>& realm_proxy() { return realm_proxy_; }

 private:
  explicit ScenicCtfTestEnvironment(fuchsia_ui_test_context::RendererType renderer_type);

  fidl::SyncClient<fuchsia_ui_test_context::ScenicRealmFactory> realm_factory_;
  fidl::SyncClient<fuchsia_ui_composition::FlatlandDisplay> flatland_display_;
  fidl::SyncClient<fuchsia_testing_harness::RealmProxy> realm_proxy_;
  std::unique_ptr<sys::ComponentContext> context_;
  const fuchsia_ui_test_context::RendererType renderer_type_;

  // The global Scenic CTF testing environment. Managed by zxtest after initial
  // creation and will be recycled after running all unit tests.
  inline static ScenicCtfTestEnvironment* global_test_environment_ = nullptr;
};

}  // namespace integration_tests

#endif  // SRC_UI_SCENIC_TESTS_UTILS_SCENIC_CTF_TEST_ENVIRONMENT_H_
