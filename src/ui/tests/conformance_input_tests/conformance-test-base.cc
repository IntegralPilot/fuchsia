// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/tests/conformance_input_tests/conformance-test-base.h"

#include <lib/component/incoming/cpp/protocol.h>

#include <utility>

namespace ui_conformance_test_base {

void ConformanceTest::SetUp() {
  auto factory_connect = component::Connect<fuchsia_ui_test_context::RealmFactory>();
  ZX_ASSERT_OK(factory_connect);
  realm_factory_ = fidl::SyncClient(std::move(factory_connect.value()));

  auto [proxy_client_end, proxy_server_end] =
      fidl::Endpoints<fuchsia_testing_harness::RealmProxy>::Create();

  fuchsia_ui_test_context::RealmFactoryCreateRealmRequest req;
  req.realm_server(std::move(proxy_server_end));
  req.display_rotation(DisplayRotation());
  req.device_pixel_ratio(DevicePixelRatio());

  ZX_ASSERT_OK(realm_factory_->CreateRealm(std::move(req)));

  realm_proxy_ = fidl::SyncClient(std::move(proxy_client_end));
}

uint32_t ConformanceTest::DisplayRotation() const { return 0; }

float ConformanceTest::DevicePixelRatio() const { return 1.f; }

}  // namespace ui_conformance_test_base
