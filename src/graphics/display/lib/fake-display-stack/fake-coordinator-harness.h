// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_COORDINATOR_HARNESS_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_COORDINATOR_HARNESS_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <memory>

#include "src/graphics/display/drivers/coordinator/controller.h"

namespace fake_display {

// This class is not thread-safe. Any concurrent access must be synchronized
// externally.
class FakeCoordinatorHarness {
 public:
  // `driver_runtime` must be non-null and must outlive
  // `FakeCoordinatorHarness`.
  FakeCoordinatorHarness(fdf_testing::DriverRuntime* driver_runtime,
                         fdf::ClientEnd<fuchsia_hardware_display_engine::Engine> engine_client);

  FakeCoordinatorHarness(const FakeCoordinatorHarness&) = delete;
  FakeCoordinatorHarness(FakeCoordinatorHarness&&) = delete;
  FakeCoordinatorHarness& operator=(const FakeCoordinatorHarness&) = delete;
  FakeCoordinatorHarness& operator=(FakeCoordinatorHarness&&) = delete;

  ~FakeCoordinatorHarness();

  // Must be called at least once.
  //
  // Shuts down and destroys the fake coordinator.
  // This method is idemponent.
  void SyncShutdown();

  // Serves coordinator services to the returned directory.
  //
  // Must not be called after `SyncShutdown()`.
  fidl::ClientEnd<fuchsia_io::Directory> Serve();

  // Serves coordinator services to the process's outgoing directory.
  //
  // Must not be called after `SyncShutdown()`.
  void ServeToProcessOutgoingDirectory();

 private:
  bool shutdown_ = false;

  fdf_testing::DriverRuntime& driver_runtime_;

  fdf::UnownedSynchronizedDispatcher coordinator_driver_dispatcher_;

  async_patterns::TestDispatcherBound<std::unique_ptr<display_coordinator::Controller>>
      coordinator_controller_;

  async_patterns::TestDispatcherBound<component::OutgoingDirectory> outgoing_;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_COORDINATOR_HARNESS_H_
