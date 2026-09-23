// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_STACK_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_STACK_H_

#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>

#include <memory>
#include <optional>

#include "src/graphics/display/lib/fake-display-stack/fake-coordinator-harness.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display-engine-harness.h"
#include "src/graphics/display/lib/fake-display-stack/sysmem-service-provider.h"

namespace fake_display {

// FakeDisplayStack creates and holds a FakeDisplay device as well as the
// Sysmem device and the display coordinator Controller which are attached to
// the fake display device and clients can connect to.
class FakeDisplayStack {
 public:
  FakeDisplayStack(std::unique_ptr<SysmemServiceProvider> sysmem_service_provider,
                   const FakeDisplayDeviceConfig& device_config);

  FakeDisplayStack(const FakeDisplayStack&) = delete;
  FakeDisplayStack(FakeDisplayStack&&) = delete;
  FakeDisplayStack& operator=(const FakeDisplayStack&) = delete;
  FakeDisplayStack& operator=(FakeDisplayStack&&) = delete;

  ~FakeDisplayStack();

  // Must not be called after SyncShutdown().
  FakeDisplay& display_engine();

  // Must not be called after SyncShutdown().
  fidl::ClientEnd<fuchsia_sysmem2::Allocator> ConnectToSysmemAllocatorV2();

  // Must be called at least once.
  //
  // Join all threads providing display and sysmem protocols, and remove all
  // the devices bound to the mock root device.
  void SyncShutdown();

  // Serves coordinator services to the returned directory.
  //
  // Must not be called after `SyncShutdown()`.
  fidl::ClientEnd<fuchsia_io::Directory> ServeCoordinator();

  // Serves coordinator services to the process's outgoing directory.
  //
  // Must not be called after `SyncShutdown()`.
  void ServeCoordinatorToProcessOutgoingDirectory();

  // Runs the driver framework foreground dispatcher until `condition` is met.
  //
  // Must not be called after `SyncShutdown()`.
  void RunDriverRuntimeDispatcherUntil(fit::function<bool()> condition);

 private:
  bool shutdown_ = false;

  std::optional<fdf_testing::ScopedGlobalLogger> logger_;

  std::shared_ptr<fdf_testing::DriverRuntime> driver_runtime_;
  std::unique_ptr<SysmemServiceProvider> sysmem_service_provider_;

  FakeDisplayEngineHarness display_engine_harness_;
  FakeCoordinatorHarness coordinator_harness_;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_STACK_H_
