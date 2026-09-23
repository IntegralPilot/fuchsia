// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/testing/base.h"

#include <fidl/fuchsia.hardware.sysmem/cpp/fidl.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <memory>
#include <utility>

#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display-device-config.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display.h"
#include "src/graphics/display/lib/fake-display-stack/fake-sysmem-device-hierarchy.h"
#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

TestBase::TestBase() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

TestBase::~TestBase() = default;

void TestBase::SetUp() {
  loop_.StartThread("display::TestBase::loop_");

  zx::result<std::unique_ptr<fake_display::FakeSysmemDeviceHierarchy>>
      create_sysmem_provider_result = fake_display::FakeSysmemDeviceHierarchy::Create();
  ASSERT_OK(create_sysmem_provider_result);
  ASSERT_NE(create_sysmem_provider_result.value(), nullptr);

  static constexpr fake_display::FakeDisplayDeviceConfig kDeviceConfig = {
      .engine_info = display::EngineInfo({
          .max_layer_count = 1,
          .max_connected_display_count = 1,
          .is_capture_supported = true,
      }),
      .periodic_vsync = false,
  };
  fake_display_stack_ = std::make_unique<fake_display::FakeDisplayStack>(
      std::move(create_sysmem_provider_result).value(), kDeviceConfig);
  incoming_root_directory_ = fake_display_stack_->ServeCoordinator();
}

void TestBase::TearDown() {
  fake_display_stack_->SyncShutdown();

  async::PostTask(loop_.dispatcher(), [this]() { loop_.Quit(); });
  loop_.JoinThreads();
}

void TestBase::WaitUntil(fit::function<bool()> predicate) {
  fake_display_stack_->RunDriverRuntimeDispatcherUntil(std::move(predicate));
}

fake_display::FakeDisplay& TestBase::FakeDisplayEngine() {
  return fake_display_stack_->display_engine();
}

fidl::ClientEnd<fuchsia_sysmem2::Allocator> TestBase::ConnectToSysmemAllocatorV2() {
  return fake_display_stack_->ConnectToSysmemAllocatorV2();
}

fidl::WireSyncClient<fuchsia_hardware_display::Provider> TestBase::DisplayProviderClient() {
  auto [incoming_svc_directory, incoming_svc_server] =
      fidl::Endpoints<fuchsia_io::Directory>::Create();

  zx_status_t open_svc_status = fdio_open3_at(incoming_root_directory_.channel().get(), "svc",
                                              uint64_t{fuchsia_io::kPermReadable},
                                              incoming_svc_server.TakeChannel().release());
  ZX_ASSERT_MSG(open_svc_status == ZX_OK, "Failed to open /svc directory: %s",
                zx_status_get_string(open_svc_status));

  component::SyncServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher(
      incoming_svc_directory.borrow());
  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> provider_result =
      watcher.GetNextInstance(/*stop_at_idle=*/false);
  ZX_ASSERT_MSG(provider_result.is_ok(), "Failed to connect to display provider: %s",
                provider_result.status_string());
  return fidl::WireSyncClient<fuchsia_hardware_display::Provider>(
      std::move(provider_result).value());
}

}  // namespace display_coordinator
