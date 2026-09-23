// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/fake-display-stack/fake-display-stack.h"

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <memory>
#include <utility>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/graphics/display/lib/fake-display-stack/fake-coordinator-harness.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display-engine-harness.h"

namespace fake_display {

FakeDisplayStack::FakeDisplayStack(std::unique_ptr<SysmemServiceProvider> sysmem_service_provider,
                                   const FakeDisplayDeviceConfig& device_config)
    : logger_(fdf::Logger::HasGlobalInstance()
                  ? std::nullopt
                  : std::make_optional<fdf_testing::ScopedGlobalLogger>()),
      driver_runtime_(mock_ddk::GetDriverRuntime()),
      sysmem_service_provider_(std::move(sysmem_service_provider)),
      display_engine_harness_(driver_runtime_.get(), ConnectToSysmemAllocatorV2(), device_config),
      coordinator_harness_(driver_runtime_.get(), display_engine_harness_.Connect()) {}

FakeDisplayStack::~FakeDisplayStack() {
  ZX_ASSERT_MSG(shutdown_, "FakeDisplayStack::SyncShutdown() not called");
}

FakeDisplay& FakeDisplayStack::display_engine() {
  ZX_ASSERT_MSG(!shutdown_, "display_engine() called after SyncShutdown()");
  return display_engine_harness_.display_engine();
}

fidl::ClientEnd<fuchsia_sysmem2::Allocator> FakeDisplayStack::ConnectToSysmemAllocatorV2() {
  ZX_ASSERT_MSG(!shutdown_, "ConnectToSysmemAllocatorV2() called after SyncShutdown()");

  zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> connect_allocator_result =
      sysmem_service_provider_->ConnectAllocator2();
  if (connect_allocator_result.is_error()) {
    ZX_PANIC("Failed to connect to sysmem Allocator service: %s",
             connect_allocator_result.status_string());
  }
  return std::move(connect_allocator_result).value();
}

void FakeDisplayStack::SyncShutdown() {
  if (shutdown_) {
    // SyncShutdown() was already called.
    return;
  }
  shutdown_ = true;

  coordinator_harness_.SyncShutdown();
  display_engine_harness_.SyncShutdown();

  sysmem_service_provider_.reset();
}

fidl::ClientEnd<fuchsia_io::Directory> FakeDisplayStack::ServeCoordinator() {
  ZX_ASSERT_MSG(!shutdown_, "SyncShutdown() called before ServeCoordinator()");
  return coordinator_harness_.Serve();
}

void FakeDisplayStack::ServeCoordinatorToProcessOutgoingDirectory() {
  ZX_ASSERT_MSG(!shutdown_,
                "SyncShutdown() called before "
                "ServeCoordinatorToProcessOutgoingDirectory()");
  coordinator_harness_.ServeToProcessOutgoingDirectory();
}

void FakeDisplayStack::RunDriverRuntimeDispatcherUntil(fit::function<bool()> condition) {
  ZX_ASSERT_MSG(!shutdown_, "RunDriverRuntimeDispatcherUntil() called after SyncShutdown()");
  driver_runtime_->RunUntil(std::move(condition));
}

}  // namespace fake_display
