// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/fake-display-stack/fake-display-engine-harness.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/fidl.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <memory>
#include <utility>

#include "src/graphics/display/lib/fake-display-stack/fake-display.h"

namespace fake_display {

FakeDisplayEngineHarness::FakeDisplayEngineHarness(
    fdf_testing::DriverRuntime* driver_runtime,
    fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client,
    const FakeDisplayDeviceConfig& device_config)
    : driver_runtime_(*driver_runtime),
      engine_driver_dispatcher_(driver_runtime_.StartBackgroundDispatcher()),
      display_engine_(std::make_unique<FakeDisplay>(&engine_events_, std::move(sysmem_client),
                                                    device_config, inspect::Inspector{})),
      fidl_adapter_(std::make_unique<display::DisplayEngineFidlAdapter>(display_engine_.get(),
                                                                        &engine_events_)) {
  ZX_ASSERT(driver_runtime != nullptr);
}

FakeDisplayEngineHarness::~FakeDisplayEngineHarness() {
  ZX_ASSERT_MSG(shutdown_, "SyncShutdown() not called before FakeDisplayEngineHarness destruction");
}

void FakeDisplayEngineHarness::SyncShutdown() {
  if (shutdown_) {
    // SyncShutdown() was already called.
    return;
  }
  shutdown_ = true;

  libsync::Completion engine_driver_dispatcher_is_shut_down;
  driver_runtime_.ShutdownBackgroundDispatcher(
      engine_driver_dispatcher_->get(), [&] { engine_driver_dispatcher_is_shut_down.Signal(); });
  engine_driver_dispatcher_is_shut_down.Wait();

  fidl_adapter_.reset();
  display_engine_.reset();
}

fdf::ClientEnd<fuchsia_hardware_display_engine::Engine> FakeDisplayEngineHarness::Connect() {
  ZX_ASSERT_MSG(!shutdown_, "Connect() called after SyncShutdown()");
  auto [engine_client, engine_server] =
      fdf::Endpoints<fuchsia_hardware_display_engine::Engine>::Create();
  zx::result<> bind_engine_result =
      fdf::RunOnDispatcherSync(engine_driver_dispatcher_->async_dispatcher(), [&]() {
        fidl::ProtocolHandler<fuchsia_hardware_display_engine::Engine> fidl_handler =
            fidl_adapter_->CreateHandler(*(engine_driver_dispatcher_->get()));
        fidl_handler(std::move(engine_server));
      });
  ZX_ASSERT_MSG(bind_engine_result.is_ok(), "Failed to handle engine protocol: %s",
                bind_engine_result.status_string());
  return std::move(engine_client);
}

}  // namespace fake_display
