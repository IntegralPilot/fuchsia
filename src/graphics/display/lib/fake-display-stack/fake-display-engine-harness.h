// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_ENGINE_HARNESS_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_ENGINE_HARNESS_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <memory>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-fidl-adapter.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display.h"

namespace fake_display {

// This class is not thread-safe. Any concurrent access must be synchronized
// externally.
class FakeDisplayEngineHarness {
 public:
  // `driver_runtime` must be non-null and must outlive
  // `FakeDisplayEngineHarness`.
  FakeDisplayEngineHarness(fdf_testing::DriverRuntime* driver_runtime,
                           fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client,
                           const FakeDisplayDeviceConfig& device_config);

  FakeDisplayEngineHarness(const FakeDisplayEngineHarness&) = delete;
  FakeDisplayEngineHarness(FakeDisplayEngineHarness&&) = delete;
  FakeDisplayEngineHarness& operator=(const FakeDisplayEngineHarness&) = delete;
  FakeDisplayEngineHarness& operator=(FakeDisplayEngineHarness&&) = delete;

  ~FakeDisplayEngineHarness();

  // Must be called at least once.
  //
  // Shuts down and destroys the fake display engine.
  // This method is idemponent.
  void SyncShutdown();

  // Must not be called after `SyncShutdown()`.
  fdf::ClientEnd<fuchsia_hardware_display_engine::Engine> Connect();

  // Must not be called after `SyncShutdown()`.
  FakeDisplay& display_engine() { return *display_engine_; }

 private:
  bool shutdown_ = false;

  fdf_testing::DriverRuntime& driver_runtime_;

  fdf::UnownedSynchronizedDispatcher engine_driver_dispatcher_;

  display::DisplayEngineEventsFidl engine_events_;

  // Created on construction. Reset during `SyncShutdown()`.
  std::unique_ptr<FakeDisplay> display_engine_;
  std::unique_ptr<display::DisplayEngineFidlAdapter> fidl_adapter_;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_ENGINE_HARNESS_H_
