// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/device-watcher/cpp/device-watcher.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace {

TEST(FakeDisplayStackHost, ConnectToServiceMemberWithListener) {
  component::SyncServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher;
  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> provider_result =
      watcher.GetNextInstance(/*stop_at_idle=*/false);
  ASSERT_OK(provider_result);

  fidl::SyncClient<fuchsia_hardware_display::Provider> provider(std::move(provider_result).value());

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();
  fidl::Result open_coordinator_result = provider->OpenCoordinator({{
      .coordinator = std::move(coordinator_server),
      .coordinator_listener = std::move(listener_client),
      .priority = fuchsia_hardware_display::kCompositorClientPriorityValue,
  }});
  EXPECT_TRUE(open_coordinator_result.is_ok())
      << "Failed to open coordinator: "
      << open_coordinator_result.error_value().FormatDescription();
}

}  // namespace
