// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/fake-display-stack/fake-coordinator-harness.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <memory>
#include <utility>

#include "src/graphics/display/drivers/coordinator/controller.h"
#include "src/graphics/display/drivers/coordinator/engine-driver-client-fidl.h"

namespace fake_display {

FakeCoordinatorHarness::FakeCoordinatorHarness(
    fdf_testing::DriverRuntime* driver_runtime,
    fdf::ClientEnd<fuchsia_hardware_display_engine::Engine> engine_client)
    : driver_runtime_(*driver_runtime),
      coordinator_driver_dispatcher_(driver_runtime_.StartBackgroundDispatcher()),
      coordinator_controller_(coordinator_driver_dispatcher_->async_dispatcher(), std::in_place),
      outgoing_(coordinator_driver_dispatcher_->async_dispatcher(), std::in_place,
                async_patterns::PassDispatcher) {
  ZX_ASSERT(driver_runtime != nullptr);

  coordinator_controller_.SyncCall(
      [&](std::unique_ptr<display_coordinator::Controller>* controller) {
        auto engine_driver_client =
            std::make_unique<display_coordinator::EngineDriverClientFidl>(std::move(engine_client));
        zx::result<std::unique_ptr<display_coordinator::Controller>> create_result =
            display_coordinator::Controller::Create(
                std::move(engine_driver_client), coordinator_driver_dispatcher_->borrow(),
                inspect::Inspector{}, /*fallback_horizontal_size_mm=*/160,
                /*fallback_vertical_size_mm=*/90);
        ZX_ASSERT_MSG(create_result.is_ok(),
                      "Failed to create display coordinator Controller device: %s",
                      create_result.status_string());
        *controller = std::move(create_result).value();
      });

  fidl::ProtocolHandler<fuchsia_hardware_display::Provider> provider_handler =
      coordinator_controller_.SyncCall(
          [&](std::unique_ptr<display_coordinator::Controller>* controller) {
            return (*controller)->bind_handler(coordinator_driver_dispatcher_->async_dispatcher());
          });
  fuchsia_hardware_display::Service::InstanceHandler service_handler({
      .provider = std::move(provider_handler),
  });
  zx::result<> add_service_result = outgoing_.SyncCall([&](component::OutgoingDirectory* outgoing) {
    return outgoing->AddService<fuchsia_hardware_display::Service>(std::move(service_handler));
  });
  ZX_ASSERT_MSG(add_service_result.is_ok(), "Failed to add display service: %s",
                add_service_result.status_string());
}

FakeCoordinatorHarness::~FakeCoordinatorHarness() {
  ZX_ASSERT_MSG(shutdown_, "SyncShutdown() not called before FakeCoordinatorHarness destruction");
}

void FakeCoordinatorHarness::SyncShutdown() {
  if (shutdown_) {
    // SyncShutdown() was already called.
    return;
  }
  shutdown_ = true;

  outgoing_.reset();

  coordinator_controller_.SyncCall(
      [&](std::unique_ptr<display_coordinator::Controller>* controller) {
        (*controller)->PrepareStop();
      });

  std::unique_ptr<display_coordinator::Controller> controller_to_reset =
      coordinator_controller_.SyncCall(
          [&](std::unique_ptr<display_coordinator::Controller>* controller) {
            return std::move(*controller);
          });

  libsync::Completion coordinator_is_shut_down;
  driver_runtime_.ShutdownBackgroundDispatcher(coordinator_driver_dispatcher_->get(), [&] {
    controller_to_reset.reset();
    coordinator_is_shut_down.Signal();
  });
  coordinator_is_shut_down.Wait();
}

fidl::ClientEnd<fuchsia_io::Directory> FakeCoordinatorHarness::Serve() {
  ZX_ASSERT_MSG(!shutdown_, "Serve() called after SyncShutdown()");
  auto [directory_client_end, directory_server_end] =
      fidl::Endpoints<fuchsia_io::Directory>::Create();
  zx::result<> serve_result =
      outgoing_.SyncCall(&component::OutgoingDirectory::Serve, std::move(directory_server_end));
  ZX_ASSERT_MSG(serve_result.is_ok(), "Failed to serve to outgoing directory: %s",
                serve_result.status_string());
  return std::move(directory_client_end);
}

void FakeCoordinatorHarness::ServeToProcessOutgoingDirectory() {
  ZX_ASSERT_MSG(!shutdown_, "ServeToProcessOutgoingDirectory() called after SyncShutdown()");
  zx::result<> serve_result =
      outgoing_.SyncCall(&component::OutgoingDirectory::ServeFromStartupInfo);
  ZX_ASSERT_MSG(serve_result.is_ok(), "Failed to serve to process outgoing directory: %s",
                serve_result.status_string());
}

}  // namespace fake_display
