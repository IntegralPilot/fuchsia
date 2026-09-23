// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/tests/utils/scenic_ctf_test_environment.h"

#include <memory>

#include <zxtest/base/runner.h>

namespace integration_tests {

void ScenicCtfTestEnvironment::RegisterGlobalTestEnvironment(
    fuchsia_ui_test_context::RendererType renderer_type) {
  global_test_environment_ = new ScenicCtfTestEnvironment(renderer_type);
  zxtest::Runner::GetInstance()->AddGlobalTestEnvironment(
      std::unique_ptr<integration_tests::ScenicCtfTestEnvironment>(global_test_environment_));
}

ScenicCtfTestEnvironment* ScenicCtfTestEnvironment::GetGlobalTestEnvironment() {
  return global_test_environment_;
}

ScenicCtfTestEnvironment::ScenicCtfTestEnvironment(
    fuchsia_ui_test_context::RendererType renderer_type)
    : renderer_type_(renderer_type) {}

void ScenicCtfTestEnvironment::SetUp() {
  context_ = sys::ComponentContext::Create();

  // Connect to realm factory.
  {
    auto result = fidl::CreateEndpoints<fuchsia_ui_test_context::ScenicRealmFactory>();
    ZX_ASSERT(result.is_ok());
    auto& [client_end, server_end] = result.value();

    const std::string& service_path =
        fuchsia_ui_test_context::ScenicRealmFactory::kDiscoverableName;
    ZX_ASSERT(context_->svc()->Connect(service_path, server_end.TakeChannel()) == ZX_OK);

    realm_factory_ =
        fidl::SyncClient<fuchsia_ui_test_context::ScenicRealmFactory>(std::move(client_end));
  }

  auto realm_result = fidl::CreateEndpoints<fuchsia_testing_harness::RealmProxy>();
  ZX_ASSERT(realm_result.is_ok());
  auto& [realm_client, realm_server] = realm_result.value();

  realm_proxy_ = fidl::SyncClient<fuchsia_testing_harness::RealmProxy>(std::move(realm_client));

  fuchsia_ui_test_context::ScenicRealmFactoryCreateRealmRequest req;
  req.realm_server() = std::move(realm_server);
  req.display_rotation() = GetDisplayRotation();
  req.renderer() = renderer_type_;
  req.display_composition() = UseDisplayComposition();
  if (GetDisplayDimensions().height() != 0 && GetDisplayDimensions().width() != 0) {
    req.display_dimensions() = GetDisplayDimensions();
  }
  if (GetDisplayRefreshRateMillihertz() != 0) {
    req.display_refresh_rate_millihertz() = GetDisplayRefreshRateMillihertz();
  }
  if (GetDisplayMaxLayerCount() != 0) {
    req.display_max_layer_count() = GetDisplayMaxLayerCount();
  }

  ZX_ASSERT(realm_factory_->CreateRealm(std::move(req)).is_ok());

  auto display_result = fidl::CreateEndpoints<fuchsia_ui_composition::FlatlandDisplay>();
  ZX_ASSERT(display_result.is_ok());
  auto& [display_client, display_server] = display_result.value();
  ZX_ASSERT(
      realm_proxy_
          ->ConnectToNamedProtocol(fuchsia_testing_harness::RealmProxyConnectToNamedProtocolRequest(
              "fuchsia.ui.composition.FlatlandDisplay", display_server.TakeChannel()))
          .is_ok());

  flatland_display_ =
      fidl::SyncClient<fuchsia_ui_composition::FlatlandDisplay>(std::move(display_client));
}

void ScenicCtfTestEnvironment::TearDown() {
  realm_proxy_ = {};
  realm_factory_ = {};
  flatland_display_ = {};
  context_.reset();
  global_test_environment_ = nullptr;
}

void ScenicCtfTestEnvironment::SetFlatlandDisplayContent(
    fuchsia_ui_views::ViewportCreationToken token) {
  auto endpoints = fidl::CreateEndpoints<fuchsia_ui_composition::ChildViewWatcher>();
  auto res = flatland_display_->SetContent(
      {{.token = std::move(token), .child_view_watcher = std::move(endpoints->server)}});
  ZX_ASSERT(res.is_ok());
}

void ScenicCtfTestEnvironment::SetFlatlandDisplayDevicePixelRatio(fuchsia_math::VecF dpr) {
  auto res = flatland_display_->SetDevicePixelRatio({{.device_pixel_ratio = dpr}});
  ZX_ASSERT(res.is_ok());
}

const std::shared_ptr<sys::ServiceDirectory>& ScenicCtfTestEnvironment::LocalServiceDirectory()
    const {
  return context_->svc();
}

uint64_t ScenicCtfTestEnvironment::GetDisplayRotation() const { return 0; }

fuchsia_math::SizeU ScenicCtfTestEnvironment::GetDisplayDimensions() const {
  return fuchsia_math::SizeU(0, 0);
}

uint32_t ScenicCtfTestEnvironment::GetDisplayRefreshRateMillihertz() const { return 0; }

uint32_t ScenicCtfTestEnvironment::GetDisplayMaxLayerCount() const { return 0; }

bool ScenicCtfTestEnvironment::UseDisplayComposition() const { return true; }

}  // namespace integration_tests
