// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_EVENTS_FIDL_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_EVENTS_FIDL_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <lib/zx/time.h>
#include <zircon/compiler.h>

#include <mutex>
#include <span>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display {

// Translates `DisplayEngineEventsInterface` C++ method calls to FIDL.
//
// This adapter targets the [`fuchsia.hardware.display.engine/EngineListener`]
// FIDL API.
//
// The adapter supports having an invalid FIDL client end. Events received while
// the client end is invalid are dropped.
//
// Instances are not thread-safe. Concurrent access must be synchronized
// externally.
class DisplayEngineEventsFidl final : public DisplayEngineEventsInterface {
 public:
  // Maximum size of the `preferred_modes` argument to `OnDisplayAdded`.
  static constexpr int kMaxPreferredModes = 32;

  // Maximum size of `pixel_formats` argument to `OnDisplayAdded`.
  static constexpr int kMaxPixelFormats = 4;

  // Creates an adapter with an invalid FIDL client end.
  explicit DisplayEngineEventsFidl();

  DisplayEngineEventsFidl(const DisplayEngineEventsFidl&) = delete;
  DisplayEngineEventsFidl& operator=(const DisplayEngineEventsFidl&) = delete;

  ~DisplayEngineEventsFidl();

  // `client_end` may be invalid.
  void SetListener(fdf::ClientEnd<fuchsia_hardware_display_engine::EngineListener> client_end);

  // DisplayEngineEventsInterface:
  void OnDisplayAdded(display::DisplayId display_id,
                      std::span<const display::ModeAndId> preferred_modes,
                      std::span<const display::PixelFormat> pixel_formats) override;
  void OnDisplayRemoved(display::DisplayId display_id) override;
  void OnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                      display::DriverConfigStamp config_stamp) override;
  void OnCaptureComplete() override;

 private:
  // TODO(fxbug.dev/473698115): This is a temporary workaround to suppress
  // races on some display engines. Instead of introducing a mutex, we should
  // revise the contract for SetListener() and all its users.
  std::mutex fidl_client_mutex_;
  fdf::WireSyncClient<fuchsia_hardware_display_engine::EngineListener> fidl_client_
      __TA_GUARDED(fidl_client_mutex_);
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_EVENTS_FIDL_H_
